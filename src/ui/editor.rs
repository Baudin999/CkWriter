use crate::app::CkWriterApp;
use crate::book::entity::EntityKind;
use crate::extract::{self, EntityHit};
use crate::llm::prompts::Pipeline;
use crate::llm::revision::{FlagKind, Revision};
use crate::theme;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::widgets::text_edit::TextEditState;
use egui::{Color32, FontFamily, FontId, Id, RichText, Stroke};

/// Per-widget layout cache stored in `egui::Memory`. Keyed by the editor's
/// `Id`; value type discriminates from `TextEditState` via `TypeId`. Lets the
/// layouter skip `build_job` on idle frames (see #0017 fix #2).
#[derive(Clone)]
struct CachedLayoutJob {
    fingerprint: u64,
    job: LayoutJob,
}

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

    let editor_id = Id::new("ckwriter-editor");

    // Pre-render: keep `entity_hits` in lockstep with `editor_text`. Without
    // this the typing frame would lay out the new text against the previous
    // frame's hits, producing a one-frame strobe on every span past the cursor
    // (see #0017). The hash compare is O(text bytes) and skips work on idle
    // frames where the buffer hasn't moved.
    let text_hash = extract::buffer_hash(&app.editor_text);
    if app.last_hits_text_hash != Some(text_hash) {
        app.refresh_entity_hits();
    }

    let font_size = app.settings.editor_font_size;
    let line_height = (font_size * LINE_HEIGHT_MULTIPLIER).round();
    let family = editor_family();
    let entity_hits = app.entity_hits.clone();
    let revisions: Vec<Revision> = app.revisions.clone();
    let selected_revision = app.selected_revision;
    let entity_hits_for_hover = entity_hits.clone();
    let revisions_for_hover = revisions.clone();
    let layout_family = family.clone();
    let family_label = family_family_label(&family);

    let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
        let fp = layout_fingerprint(&LayoutInputs {
            text,
            hits: &entity_hits,
            revisions: &revisions,
            selected_revision,
            font_size,
            line_height,
            family_label: &family_label,
            wrap_width,
        });
        let cached = ui.ctx().memory_mut(|mem| {
            mem.data
                .get_temp::<CachedLayoutJob>(editor_id)
                .filter(|c| c.fingerprint == fp)
                .map(|c| c.job)
        });
        let job = if let Some(j) = cached {
            j
        } else {
            #[cfg(debug_assertions)]
            log::trace!(
                "editor build_job: fp={fp:#x} text_len={} hits={} revs={}",
                text.len(),
                entity_hits.len(),
                revisions.len(),
            );
            let mut j = build_job(
                text,
                font_size,
                line_height,
                &layout_family,
                &entity_hits,
                &revisions,
                selected_revision,
            );
            j.wrap.max_width = wrap_width;
            let stored = CachedLayoutJob {
                fingerprint: fp,
                job: j.clone(),
            };
            ui.ctx().memory_mut(|mem| {
                mem.data.insert_temp(editor_id, stored);
            });
            j
        };
        ui.fonts(|f| f.layout_job(job))
    };

    // Pick the scroll offset for this frame: a jump-to-source line wins over a
    // chapter-restore offset; chapter-restore is consumed otherwise. Cursor
    // restore is only honoured if there's no jump (a jump owns the viewport).
    let scroll_target = if let Some(line) = app.pending_scroll_line.take() {
        app.pending_scroll_offset = None;
        app.pending_cursor_char = None;
        Some((line as f32 * line_height - line_height * 4.0).max(0.0))
    } else {
        app.pending_scroll_offset.take()
    };
    let cursor_to_install = app.pending_cursor_char.take();
    if let Some(idx) = cursor_to_install {
        let mut state = TextEditState::load(ui.ctx(), editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(idx))));
        state.store(ui.ctx(), editor_id);
    }
    // Consume now so the post-render block knows whether to scroll the
    // cursor into view this frame, and which char to scroll to. We can't rely
    // on `output.cursor_range` because egui only populates it when the
    // TextEdit has focus (builder.rs gates it on `mem.has_focus(id)`), and
    // clicking an AI card leaves focus on the panel.
    let scroll_to_cursor_char = if std::mem::take(&mut app.pending_scroll_to_cursor) {
        cursor_to_install
    } else {
        None
    };

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

                // After the TextEdit has rendered, we know exactly where the
                // target char sits in the wrapped galley. Translate that
                // local rect into screen coords and ask the parent ScrollArea
                // to bring it on-screen — this is the one path that handles
                // soft-wrapped LaTeX paragraphs correctly. We compute from
                // the CCursor directly (not `output.cursor_range`) because
                // egui only populates `cursor_range` for a focused TextEdit;
                // clicks from the AI panel leave focus on the panel.
                if let Some(idx) = scroll_to_cursor_char {
                    let local_rect = output.galley.pos_from_ccursor(CCursor::new(idx));
                    let screen_rect = local_rect.translate(output.galley_pos.to_vec2());
                    log::info!(
                        "editor scroll_to_cursor: ccursor={idx} local_rect={local_rect:?} galley_pos={:?} screen_rect={screen_rect:?}",
                        output.galley_pos,
                    );
                    ui.scroll_to_rect(screen_rect, Some(egui::Align::Center));
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
                                r.anchor.map(|(s, e)| byte >= s && byte < e).unwrap_or(false)
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
        Pipeline::Spelling => theme::REVISION_SPELLING,
    }
}

/// The colour used for a revision's underline + card chip. Spelling-pipeline
/// flags split into spelling/punctuation/grammar; everything else falls back
/// to its pipeline's colour.
pub fn revision_color(rev: &Revision) -> Color32 {
    match rev.kind {
        FlagKind::Spelling => theme::REVISION_SPELLING,
        FlagKind::Punctuation => theme::REVISION_PUNCTUATION,
        FlagKind::Grammar => theme::REVISION_GRAMMAR,
        FlagKind::Other => pipeline_color(rev.pipeline),
    }
}

fn build_job(
    text: &str,
    font_size: f32,
    line_height: f32,
    family: &FontFamily,
    hits: &[EntityHit],
    revisions: &[Revision],
    selected_revision: Option<u32>,
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
    for r in revisions {
        if let Some((s, e)) = r.anchor {
            let color = revision_color(r);
            let selected = selected_revision == Some(r.id);
            let mut f = base.clone();
            // Selected revision wins visually: thicker underline + tinted
            // background so the writer can spot the active edit at a glance.
            if selected {
                f.underline = Stroke::new(3.0, color);
                f.background = theme::REVISION_SELECTED_BG;
            } else {
                f.underline = Stroke::new(2.0, color);
            }
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

/// Stable label for `FontFamily` fingerprinting. Family is `'static` per
/// session today (theme/font are settings-driven, not text-driven), so this
/// is mostly defensive — but if we ever swap families on the fly the cache
/// must miss.
fn family_family_label(f: &FontFamily) -> String {
    match f {
        FontFamily::Name(n) => n.to_string(),
        FontFamily::Monospace => "<monospace>".to_string(),
        FontFamily::Proportional => "<proportional>".to_string(),
    }
}

fn entity_kind_byte(k: EntityKind) -> u8 {
    match k {
        EntityKind::Character => 0,
        EntityKind::Location => 1,
        EntityKind::Event => 2,
        EntityKind::Timeline => 3,
    }
}

fn flag_kind_byte(k: FlagKind) -> u8 {
    match k {
        FlagKind::Spelling => 0,
        FlagKind::Punctuation => 1,
        FlagKind::Grammar => 2,
        FlagKind::Other => 3,
    }
}

fn pipeline_byte(p: Pipeline) -> u8 {
    match p {
        Pipeline::Voice => 0,
        Pipeline::ShowDontTell => 1,
        Pipeline::Prose => 2,
        Pipeline::Spelling => 3,
    }
}

/// Every input that can change the laid-out galley for one frame. Grouped
/// into a struct so the fingerprint and the layouter share one definition of
/// "what counts as the same layout."
struct LayoutInputs<'a> {
    text: &'a str,
    hits: &'a [EntityHit],
    revisions: &'a [Revision],
    selected_revision: Option<u32>,
    font_size: f32,
    line_height: f32,
    family_label: &'a str,
    wrap_width: f32,
}

/// Produce a 64-bit fingerprint over every input that can change the laid-out
/// galley for this frame. Equality of fingerprints implies equality of the
/// resulting `LayoutJob`; the layouter uses this to short-circuit `build_job`
/// when none of the inputs moved (see #0017 fix #2).
///
/// We hand-hash the structured inputs rather than rely on `Hash` derivations
/// on third-party types (`LayoutJob`, `EntityHit`, `Revision`) so the contract
/// is explicit. Floats are hashed via `to_bits` (NaN-safe and bit-stable).
fn layout_fingerprint(inp: &LayoutInputs<'_>) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(b"text\0");
    h.update(blake3::hash(inp.text.as_bytes()).as_bytes());

    h.update(b"hits\0");
    h.update(&(inp.hits.len() as u64).to_le_bytes());
    for hit in inp.hits {
        h.update(&(hit.start as u64).to_le_bytes());
        h.update(&(hit.end as u64).to_le_bytes());
        h.update(&(hit.entity_id.len() as u32).to_le_bytes());
        h.update(hit.entity_id.as_bytes());
        h.update(&[entity_kind_byte(hit.kind)]);
    }

    h.update(b"revs\0");
    h.update(&(inp.revisions.len() as u64).to_le_bytes());
    for r in inp.revisions {
        h.update(&r.id.to_le_bytes());
        match r.anchor {
            Some((s, e)) => {
                h.update(&[1u8]);
                h.update(&(s as u64).to_le_bytes());
                h.update(&(e as u64).to_le_bytes());
            }
            None => {
                h.update(&[0u8]);
            }
        }
        h.update(&[flag_kind_byte(r.kind)]);
        h.update(&[pipeline_byte(r.pipeline)]);
    }

    h.update(b"sel\0");
    match inp.selected_revision {
        Some(id) => {
            h.update(&[1u8]);
            h.update(&id.to_le_bytes());
        }
        None => {
            h.update(&[0u8]);
        }
    }

    h.update(b"fmt\0");
    h.update(&inp.font_size.to_bits().to_le_bytes());
    h.update(&inp.line_height.to_bits().to_le_bytes());
    h.update(&(inp.family_label.len() as u32).to_le_bytes());
    h.update(inp.family_label.as_bytes());
    h.update(&inp.wrap_width.to_bits().to_le_bytes());

    let out = h.finalize();
    let bytes = out.as_bytes();
    u64::from_le_bytes(bytes[..8].try_into().expect("blake3 hash is 32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hit(id: &str) -> EntityHit {
        EntityHit {
            start: 10,
            end: 20,
            entity_id: id.to_string(),
            kind: EntityKind::Character,
        }
    }

    fn sample_revision(id: u32) -> Revision {
        Revision {
            id,
            pipeline: Pipeline::Voice,
            kind: FlagKind::Other,
            quote: "q".to_string(),
            why: "w".to_string(),
            suggestion: "s".to_string(),
            anchor: Some((30, 40)),
            suggestion_id: "abc".to_string(),
            paragraph_id: None,
            is_dismissed: false,
        }
    }

    /// Owned mirror of `LayoutInputs` so tests can mutate fields between
    /// calls. `view()` produces a borrowed `LayoutInputs` for the fingerprint.
    struct OwnedInputs {
        text: String,
        hits: Vec<EntityHit>,
        revisions: Vec<Revision>,
        selected_revision: Option<u32>,
        font_size: f32,
        line_height: f32,
        family_label: String,
        wrap_width: f32,
    }

    impl OwnedInputs {
        fn baseline() -> Self {
            Self {
                text: "the prose under test".to_string(),
                hits: vec![sample_hit("char-1")],
                revisions: vec![sample_revision(7)],
                selected_revision: Some(7),
                font_size: 18.0,
                line_height: 30.0,
                family_label: "writer".to_string(),
                wrap_width: 720.0,
            }
        }

        fn view(&self) -> LayoutInputs<'_> {
            LayoutInputs {
                text: &self.text,
                hits: &self.hits,
                revisions: &self.revisions,
                selected_revision: self.selected_revision,
                font_size: self.font_size,
                line_height: self.line_height,
                family_label: &self.family_label,
                wrap_width: self.wrap_width,
            }
        }

        fn fp(&self) -> u64 {
            layout_fingerprint(&self.view())
        }
    }

    #[test]
    fn identical_inputs_produce_identical_fingerprint() {
        let a = OwnedInputs::baseline();
        let b = OwnedInputs::baseline();
        assert_eq!(a.fp(), b.fp());
    }

    #[test]
    fn text_perturbation_changes_fingerprint() {
        let base_fp = OwnedInputs::baseline().fp();
        let mut alt = OwnedInputs::baseline();
        alt.text.push('!');
        assert_ne!(base_fp, alt.fp());
    }

    #[test]
    fn hit_perturbation_changes_fingerprint() {
        let base_fp = OwnedInputs::baseline().fp();

        let mut start_changed = OwnedInputs::baseline();
        start_changed.hits[0].start += 1;
        assert_ne!(base_fp, start_changed.fp());

        let mut end_changed = OwnedInputs::baseline();
        end_changed.hits[0].end += 1;
        assert_ne!(base_fp, end_changed.fp());

        let mut id_changed = OwnedInputs::baseline();
        id_changed.hits[0].entity_id = "char-2".into();
        assert_ne!(base_fp, id_changed.fp());

        let mut kind_changed = OwnedInputs::baseline();
        kind_changed.hits[0].kind = EntityKind::Location;
        assert_ne!(base_fp, kind_changed.fp());

        let mut len_changed = OwnedInputs::baseline();
        len_changed.hits.push(sample_hit("char-2"));
        assert_ne!(base_fp, len_changed.fp());
    }

    #[test]
    fn revision_perturbation_changes_fingerprint() {
        let base_fp = OwnedInputs::baseline().fp();

        let mut id_changed = OwnedInputs::baseline();
        id_changed.revisions[0].id = 9;
        assert_ne!(base_fp, id_changed.fp());

        let mut anchor_changed = OwnedInputs::baseline();
        anchor_changed.revisions[0].anchor = Some((31, 41));
        assert_ne!(base_fp, anchor_changed.fp());

        let mut anchor_dropped = OwnedInputs::baseline();
        anchor_dropped.revisions[0].anchor = None;
        assert_ne!(base_fp, anchor_dropped.fp());

        let mut kind_changed = OwnedInputs::baseline();
        kind_changed.revisions[0].kind = FlagKind::Spelling;
        assert_ne!(base_fp, kind_changed.fp());

        let mut pipeline_changed = OwnedInputs::baseline();
        pipeline_changed.revisions[0].pipeline = Pipeline::Prose;
        assert_ne!(base_fp, pipeline_changed.fp());
    }

    #[test]
    fn selected_revision_perturbation_changes_fingerprint() {
        let base_fp = OwnedInputs::baseline().fp();

        let mut none = OwnedInputs::baseline();
        none.selected_revision = None;
        assert_ne!(base_fp, none.fp());

        let mut other = OwnedInputs::baseline();
        other.selected_revision = Some(8);
        assert_ne!(base_fp, other.fp());
    }

    #[test]
    fn font_layout_perturbation_changes_fingerprint() {
        let base_fp = OwnedInputs::baseline().fp();

        let mut size = OwnedInputs::baseline();
        size.font_size = 19.0;
        assert_ne!(base_fp, size.fp());

        let mut line = OwnedInputs::baseline();
        line.line_height = 31.0;
        assert_ne!(base_fp, line.fp());

        let mut fam = OwnedInputs::baseline();
        fam.family_label = "monospace".into();
        assert_ne!(base_fp, fam.fp());

        let mut wrap = OwnedInputs::baseline();
        wrap.wrap_width = 700.0;
        assert_ne!(base_fp, wrap.fp());
    }

    #[test]
    fn empty_inputs_are_stable() {
        let inp = LayoutInputs {
            text: "",
            hits: &[],
            revisions: &[],
            selected_revision: None,
            font_size: 16.0,
            line_height: 24.0,
            family_label: "",
            wrap_width: 600.0,
        };
        assert_eq!(layout_fingerprint(&inp), layout_fingerprint(&inp));
    }
}
