use crate::app::CkWriterApp;
use crate::book::entity::EntityKind;
use crate::extract::{self, EntityHit};
use crate::llm::prompts::Pipeline;
use crate::llm::revision::{Revision, RevisionStatus};
use crate::theme;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Pos2, RichText, Stroke};

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
    let entity_hits = app.entity_hits.clone();
    let revisions: Vec<Revision> = app.revisions.clone();
    let entity_hits_for_hover = entity_hits.clone();
    let revisions_for_hover = revisions.clone();

    let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
        let mut job = build_job(text, font_size, &entity_hits, &revisions);
        job.wrap.max_width = wrap_width;
        ui.fonts(|f| f.layout_job(job))
    };

    let row_height =
        ui.fonts(|f| f.row_height(&FontId::new(font_size, egui::FontFamily::Monospace)));
    let scroll_target = app
        .pending_scroll_line
        .take()
        .map(|line| (line as f32 * row_height - row_height * 4.0).max(0.0));

    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false; 2]);
    if let Some(off) = scroll_target {
        scroll = scroll.vertical_scroll_offset(off);
    }
    scroll.show(ui, |ui| {
            let edit = egui::TextEdit::multiline(&mut app.editor_text)
                .font(FontId::new(font_size, egui::FontFamily::Monospace))
                .desired_width(f32::INFINITY)
                .desired_rows(28)
                .layouter(&mut layouter);
            let response = ui.add_sized(ui.available_size(), edit);

            if response.changed() {
                app.dirty = true;
            }

            // Hover detection (after the TextEdit borrow has released).
            if let Some(pointer) = ui.ctx().pointer_hover_pos() {
                if response.rect.contains(pointer) {
                    if let Some(byte) =
                        pointer_to_byte(&response, ui, pointer, &app.editor_text, font_size)
                    {
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
            }
        });

    if app.dirty {
        app.refresh_entity_hits();
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

fn build_job(text: &str, font_size: f32, hits: &[EntityHit], revisions: &[Revision]) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base = TextFormat {
        font_id: FontId::new(font_size, egui::FontFamily::Monospace),
        color: theme::TEXT_PRIMARY,
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

fn pointer_to_byte(
    response: &egui::Response,
    ui: &egui::Ui,
    pointer: Pos2,
    text: &str,
    font_size: f32,
) -> Option<usize> {
    let row_height =
        ui.fonts(|f| f.row_height(&FontId::new(font_size, egui::FontFamily::Monospace)));
    let rel = pointer - response.rect.min;
    if rel.x < 0.0 || rel.y < 0.0 {
        return None;
    }
    let row = (rel.y / row_height).floor() as usize;

    // Walk lines to find row_start byte index.
    let mut current_row = 0usize;
    let mut row_start = 0usize;
    let mut found = false;
    if row == 0 {
        found = true;
    } else {
        for (idx, ch) in text.char_indices() {
            if ch == '\n' {
                current_row += 1;
                if current_row == row {
                    row_start = idx + 1;
                    found = true;
                    break;
                }
            }
        }
    }
    if !found {
        return None;
    }

    let char_w = ui.fonts(|f| {
        f.glyph_width(&FontId::new(font_size, egui::FontFamily::Monospace), 'M')
    });
    let col = (rel.x / char_w.max(1.0)).round() as usize;

    let mut visible = 0usize;
    for (off, ch) in text[row_start..].char_indices() {
        if visible >= col {
            return Some(row_start + off);
        }
        if ch == '\n' {
            return Some(row_start + off);
        }
        visible += 1;
    }
    Some(text.len())
}
