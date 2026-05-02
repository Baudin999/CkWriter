use crate::app::CkWriterApp;
use crate::book::entity::EntityKind;
use crate::extract::{self, EntityHit};
use crate::llm::prompts::Pipeline;
use crate::llm::revision::{Revision, RevisionStatus};
use crate::theme;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::widgets::text_edit::TextEditState;
use egui::{Color32, FontFamily, FontId, Id, RichText, Stroke};

const MAX_COLUMN_WIDTH: f32 = 760.0;
const MIN_COLUMN_WIDTH: f32 = 360.0;
const MIN_SIDE_PADDING: f32 = 24.0;
const TOP_PADDING: f32 = 32.0;
const BOTTOM_PADDING: f32 = 96.0;
const LINE_HEIGHT_MULTIPLIER: f32 = 1.7;

fn editor_family() -> FontFamily {
    FontFamily::Name(theme::WRITER_FAMILY.into())
}

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.book.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("Open a book to start writing.")
                    .color(theme::TEXT_MUTED)
                    .size(18.0),
            );
        });
        return;
    }

    if app.current_chapter.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("Select a chapter from the left.")
                    .color(theme::TEXT_MUTED)
                    .size(16.0),
            );
        });
        return;
    }

    let font_size = app.settings.editor_font_size;
    let line_height = (font_size * LINE_HEIGHT_MULTIPLIER).round();
    let family = editor_family();
    let entity_hits = app.entity_hits.clone();
    let revisions: Vec<Revision> = app.revisions.clone();
    let entity_hits_for_hover = entity_hits.clone();
    let revisions_for_hover = revisions.clone();
    let layout_family = family.clone();

    let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
        let mut job = build_job(
            text,
            font_size,
            line_height,
            &layout_family,
            &entity_hits,
            &revisions,
        );
        job.wrap.max_width = wrap_width;
        ui.fonts(|f| f.layout_job(job))
    };

    // Pick the scroll offset for this frame: a jump-to-source line wins over a
    // chapter-restore offset; chapter-restore is consumed otherwise. Cursor
    // restore is only honoured if there's no jump (a jump owns the viewport).
    let editor_id = Id::new("ckwriter-editor");
    let scroll_target = if let Some(line) = app.pending_scroll_line.take() {
        app.pending_scroll_offset = None;
        app.pending_cursor_char = None;
        Some((line as f32 * line_height - line_height * 4.0).max(0.0))
    } else {
        app.pending_scroll_offset.take()
    };
    if let Some(idx) = app.pending_cursor_char.take() {
        let mut state = TextEditState::load(ui.ctx(), editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(idx))));
        state.store(ui.ctx(), editor_id);
    }

    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false; 2]);
    if let Some(off) = scroll_target {
        scroll = scroll.vertical_scroll_offset(off);
    }
    let scroll_out = scroll.show(ui, |ui| {
        let avail = ui.available_size();
        let pad_x = (((avail.x - MAX_COLUMN_WIDTH) * 0.5).max(MIN_SIDE_PADDING)).floor();
        let column_w = (avail.x - 2.0 * pad_x).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
        let rows = ((avail.y / line_height).floor() as usize).max(8);

        ui.add_space(TOP_PADDING);
        let mut cursor_char: Option<usize> = None;
        ui.horizontal(|ui| {
            ui.add_space(pad_x);
            ui.vertical(|ui| {
                let edit = egui::TextEdit::multiline(&mut app.editor_text)
                    .id(editor_id)
                    .font(FontId::new(font_size, family.clone()))
                    .desired_width(column_w)
                    .desired_rows(rows)
                    .frame(false)
                    .margin(egui::Margin::symmetric(0, 4))
                    .layouter(&mut layouter);
                let output = edit.show(ui);
                let response = &output.response;

                if response.changed() {
                    app.dirty = true;
                }

                if let Some(range) = output.cursor_range {
                    cursor_char = Some(range.primary.ccursor.index);
                }

                // Hover detection: ask the rendered galley directly so wrapping is honoured.
                if let Some(pointer) = response.hover_pos() {
                    let local = pointer - output.galley_pos;
                    if output.galley.rect.contains(local.to_pos2()) {
                        let cursor = output.galley.cursor_from_pos(local);
                        let byte = char_to_byte(&app.editor_text, cursor.ccursor.index);
                        let rev = revisions_for_hover
                            .iter()
                            .find(|r| {
                                r.status == RevisionStatus::Pending
                                    && r.anchor.map(|(s, e)| byte >= s && byte < e).unwrap_or(false)
                            })
                            .cloned();
                        if let Some(rev) = rev {
                            show_revision_tooltip(ui, &rev);
                        } else if let Some(hit) = extract::hit_at(&entity_hits_for_hover, byte) {
                            show_entity_tooltip(app, ui, hit);
                        }
                    }
                }
            });
            ui.add_space(pad_x);
        });
        ui.add_space(BOTTOM_PADDING);
        cursor_char
    });

    if app.dirty {
        app.refresh_entity_hits();
    }

    // Persist the reading position for this chapter. Saved values are debounced
    // through `settings_dirty`; the periodic save in `app.update` handles flushing.
    if let Some(ch) = app.current_chapter.as_ref() {
        let path = ch.file_path.clone();
        let scroll_y = scroll_out.state.offset.y;
        let new_cursor = scroll_out.inner.unwrap_or_else(|| {
            app.settings
                .chapter_places
                .get(&path)
                .map(|p| p.cursor)
                .unwrap_or(0)
        });
        let entry = app.settings.chapter_places.entry(path).or_default();
        let changed = entry.cursor != new_cursor || (entry.scroll - scroll_y).abs() > 1.0;
        entry.cursor = new_cursor;
        entry.scroll = scroll_y;
        if changed {
            app.settings_dirty = true;
        }
    }
}

fn show_entity_tooltip(app: &CkWriterApp, ui: &egui::Ui, hit: &EntityHit) {
    let Some(book) = &app.book else { return };
    let Some(e) = book.entity(&hit.entity_id) else {
        return;
    };
    egui::show_tooltip_at_pointer(
        ui.ctx(),
        ui.layer_id(),
        egui::Id::new(("entity-hover", &e.id)),
        |ui| {
            ui.label(RichText::new(&e.name).strong().size(14.0));
            if !e.role.is_empty() {
                ui.label(RichText::new(&e.role).color(theme::TEXT_MUTED));
            }
            if !e.age.is_empty() {
                ui.label(format!("age: {}", e.age));
            }
            if !e.tone.is_empty() {
                ui.label(format!("tone: {}", e.tone));
            }
            if !e.voice_notes.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new(&e.voice_notes).italics());
            }
            if !e.relations.is_empty() {
                ui.add_space(2.0);
                ui.label(RichText::new("relations").small().color(theme::TEXT_MUTED));
                for r in &e.relations {
                    ui.label(format!("  · {}: {}", r.kind, r.id));
                }
            }
        },
    );
}

fn show_revision_tooltip(ui: &egui::Ui, rev: &Revision) {
    let color = pipeline_color(rev.pipeline);
    egui::show_tooltip_at_pointer(
        ui.ctx(),
        ui.layer_id(),
        egui::Id::new(("rev-hover", rev.id)),
        |ui| {
            ui.label(RichText::new(rev.pipeline.label()).color(color).strong());
            ui.label(RichText::new(&rev.why));
            if !rev.suggestion.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("suggestion").small().color(theme::TEXT_MUTED));
                ui.label(RichText::new(&rev.suggestion).italics());
            }
        },
    );
}

fn pipeline_color(p: Pipeline) -> Color32 {
    match p {
        Pipeline::Voice => theme::REVISION_VOICE,
        Pipeline::ShowDontTell => theme::REVISION_SHOW,
        Pipeline::Prose => theme::REVISION_PROSE,
    }
}

fn build_job(
    text: &str,
    font_size: f32,
    line_height: f32,
    family: &FontFamily,
    hits: &[EntityHit],
    revisions: &[Revision],
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base = TextFormat {
        font_id: FontId::new(font_size, family.clone()),
        color: theme::TEXT_PRIMARY,
        line_height: Some(line_height),
        extra_letter_spacing: 0.1,
        ..Default::default()
    };

    let mut spans: Vec<(usize, usize, TextFormat)> = Vec::new();
    for h in hits {
        let color = match h.kind {
            EntityKind::Character => theme::ENTITY_CHARACTER,
            EntityKind::Location => theme::ENTITY_LOCATION,
            _ => theme::TEXT_PRIMARY,
        };
        let mut f = base.clone();
        f.color = color;
        f.underline = Stroke::new(1.0, color.linear_multiply(0.6));
        spans.push((h.start, h.end, f));
    }
    for r in revisions.iter().filter(|r| r.status == RevisionStatus::Pending) {
        if let Some((s, e)) = r.anchor {
            let mut f = base.clone();
            f.underline = Stroke::new(2.0, pipeline_color(r.pipeline));
            spans.push((s, e, f));
        }
    }
    spans.sort_by_key(|(s, _, _)| *s);

    let mut cursor = 0usize;
    for (s, e, fmt) in spans {
        if s < cursor || e > text.len() || s >= e {
            continue;
        }
        if cursor < s {
            job.append(&text[cursor..s], 0.0, base.clone());
        }
        job.append(&text[s..e], 0.0, fmt);
        cursor = e;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, base);
    }
    job
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}
