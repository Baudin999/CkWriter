use crate::app::CkWriterApp;
use crate::book::entity::EntityKind;
use crate::book::paragraphs::Paragraph;
use crate::extract::{self, EntityHit};
use crate::icons;
use crate::llm::prompts::Pipeline;
use crate::llm::revision::{FlagKind, Revision};
use crate::theme;
use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::widgets::text_edit::TextEditState;
use egui::{Align2, Color32, FontFamily, FontId, Id, RichText, Sense, Stroke};
use std::collections::BTreeMap;

/// Per-widget layout cache stored in `egui::Memory`. Keyed by the editor's
/// `Id`; value type discriminates from `TextEditState` via `TypeId`. Lets the
/// layouter skip `build_job` on idle frames (see #0017 fix #2).
#[derive(Clone)]
struct CachedLayoutJob {
    fingerprint: u64,
    job: LayoutJob,
}

/// Per-editor multi-click counter (#0016). Lives in `egui::Memory`, keyed by
/// the editor `Id` extended with `"multi-click"`. Persists across frames but
/// not across windows; egui's own counter caps at 3, so this is the source of
/// truth for the 4-click paragraph step.
#[derive(Clone, Copy)]
struct MultiClickState {
    last_time: f64,
    last_pos: egui::Pos2,
    count: u32,
}

/// Lower bound for the editor's prose column. The upper bound now comes from
/// `Settings::editor_column_width` (#0020 pivot); this responsive minimum
/// kicks in only when the window itself is narrower than the user's chosen
/// width.
const MIN_COLUMN_WIDTH: f32 = 360.0;
/// Bumped 24 → 56 across #0024 (play) and #0025 (trash) to fit two
/// hover-only gutter glyphs between the page edge and the dirty bar.
/// Layout, left → right: play | gap | trash | gap | bar | gap | prose.
const MIN_SIDE_PADDING: f32 = 56.0;
const TOP_PADDING: f32 = 32.0;
const BOTTOM_PADDING: f32 = 96.0;

/// Width of the per-paragraph dirty gutter painted to the left of the editor
/// column (#0023). Sits inside the column's left padding, with `GUTTER_GAP_PX`
/// of breathing room between the gutter and the prose.
const GUTTER_WIDTH_PX: f32 = 3.0;
const GUTTER_GAP_PX: f32 = 8.0;

/// Hover-only per-paragraph control glyphs (#0024 play, #0025 clear).
/// `ICON_GAP_PX` is the spacing between adjacent icons and between the
/// rightmost icon and the dirty bar.
const ICON_SIZE_PX: f32 = 14.0;
const ICON_GAP_PX: f32 = 6.0;

/// Time window for chained clicks to count as the same multi-click sequence
/// (#0016). Slightly longer than egui's own 0.3 s default so 4-click reaches
/// users with less practiced timing — the paragraph step is the new gesture
/// and should be the easiest to land.
const MULTI_CLICK_WINDOW_SECS: f64 = 0.4;
/// Pointer drift tolerance between consecutive clicks before the counter
/// resets (#0016). Matches egui's "is this still a click" feel — small enough
/// that a deliberate move-and-click anywhere else starts fresh, large enough
/// that hand jitter on a fast 4-click never breaks the chain.
const MULTI_CLICK_RADIUS_PX: f32 = 3.0;

/// Pipeline labels considered for the per-paragraph dirty gutter. Voice is
/// chapter-level so it's excluded by design (#0023). Kept in sync with
/// `Pipeline::label`.
const GUTTER_PIPELINE_LABELS: &[&str] = &["show, don't tell", "prose", "spelling"];

fn editor_family(app: &CkWriterApp) -> FontFamily {
    theme::reading_family(app.settings.reading_font)
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

    let font_size = app.settings.font_size_normal;
    let line_height = (font_size * app.settings.editor_line_height_mult).round();
    let letter_spacing = app.settings.editor_letter_spacing;
    let max_column_width = app.settings.editor_column_width;
    let family = editor_family(app);
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
            letter_spacing,
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
            let style = ReadingStyle {
                font_size,
                line_height,
                letter_spacing,
                family: &layout_family,
            };
            let mut j = build_job(
                text,
                &style,
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

    // Snapshot the per-paragraph gutter state before the closure takes a
    // mutable borrow on `app.editor_text`. Each entry pairs the resolved
    // state with the paragraph's `char_range` from the saved index — which
    // may be stale relative to unsaved keystrokes by design (#0023): the
    // gutter answers "what will the next coach run cost? what's pending?",
    // a save-time question.
    let paragraph_gutter_marks: Vec<(String, GutterState, (usize, usize))> = app
        .current_chapter
        .as_ref()
        .map(|ch| {
            app.current_paragraphs
                .iter()
                .map(|p| {
                    let state = gutter_state_for(p, &ch.meta.last_run_hashes, &app.revisions);
                    (p.id.clone(), state, p.char_range)
                })
                .collect()
        })
        .unwrap_or_default();

    let mut scroll = egui::ScrollArea::vertical().auto_shrink([false; 2]);
    if let Some(off) = scroll_target {
        scroll = scroll.vertical_scroll_offset(off);
    }
    let scroll_out = scroll.show(ui, |ui| {
        let avail = ui.available_size();
        let pad_x = (((avail.x - max_column_width) * 0.5).max(MIN_SIDE_PADDING)).floor();
        let column_w = (avail.x - 2.0 * pad_x).clamp(MIN_COLUMN_WIDTH, max_column_width);
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

                // 4-click → select paragraph (#0016). 1/2/3 clicks are handled
                // by egui's TextEdit (cursor / word / line). egui caps its
                // own click counter at 3, so for the paragraph step we keep a
                // tiny counter in `Memory` keyed on the editor `Id`. Reset
                // after `MULTI_CLICK_WINDOW_SECS` of pointer idle, or when
                // the click drifts more than `MULTI_CLICK_RADIUS_PX` from
                // the prior position. Paragraph boundaries are blank lines
                // OR the literal `\nl` token — see `paragraph_char_range_at`.
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let now = ui.ctx().input(|i| i.time);
                        let multi_id = editor_id.with("multi-click");
                        let prior: Option<MultiClickState> = ui
                            .ctx()
                            .memory(|m| m.data.get_temp(multi_id));
                        let count = match prior {
                            Some(s)
                                if now - s.last_time < MULTI_CLICK_WINDOW_SECS
                                    && s.last_pos.distance(pos)
                                        < MULTI_CLICK_RADIUS_PX =>
                            {
                                s.count + 1
                            }
                            _ => 1,
                        };
                        ui.ctx().memory_mut(|m| {
                            m.data.insert_temp(
                                multi_id,
                                MultiClickState {
                                    last_time: now,
                                    last_pos: pos,
                                    count,
                                },
                            );
                        });

                        if count == 4 {
                            let local = pos - output.galley_pos;
                            if output.galley.rect.contains(local.to_pos2()) {
                                let cursor =
                                    output.galley.cursor_from_pos(local);
                                let click_char = cursor.ccursor.index;
                                let (start_char, end_char) =
                                    paragraph_char_range_at(
                                        &app.editor_text,
                                        click_char,
                                    );
                                let mut state = TextEditState::load(
                                    ui.ctx(),
                                    editor_id,
                                )
                                .unwrap_or_default();
                                state.cursor.set_char_range(Some(
                                    CCursorRange::two(
                                        CCursor::new(start_char),
                                        CCursor::new(end_char),
                                    ),
                                ));
                                state.store(ui.ctx(), editor_id);
                            }
                        }
                    }
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

                // Paint per-paragraph state markers in the left margin
                // (#0023). The gutter sits just outside the prose column,
                // between the page padding and the first glyph — so it shares
                // the column's scroll without competing with line-wrapped
                // text. Pixel positions come from the laid-out galley, so
                // wrapping is honoured automatically. Every paragraph paints,
                // including Clean (gray) — the constant scaffold makes
                // transitions to yellow/orange/red read as state changes
                // against a stable backdrop.
                let mut play_clicked: Option<String> = None;
                let mut clear_clicked: Option<String> = None;
                if !paragraph_gutter_marks.is_empty() {
                    let gutter_x =
                        output.galley_pos.x - GUTTER_GAP_PX - GUTTER_WIDTH_PX * 0.5;
                    let bar_left_edge =
                        output.galley_pos.x - GUTTER_GAP_PX - GUTTER_WIDTH_PX;
                    // Layout left→right: play | gap | trash | gap | bar.
                    let trash_icon_x = bar_left_edge - ICON_GAP_PX - ICON_SIZE_PX * 0.5;
                    let play_icon_x =
                        trash_icon_x - ICON_SIZE_PX * 0.5 - ICON_GAP_PX - ICON_SIZE_PX * 0.5;

                    // Resolve the hovered paragraph by Y-band so the pointer
                    // can sit in the gutter (or on a glyph itself) without
                    // dismissing the icons. Y comparison in galley-local
                    // space; translates to the same screen-space the
                    // painter uses.
                    let pointer_y_local = ui
                        .ctx()
                        .input(|i| i.pointer.hover_pos())
                        .map(|p| p.y - output.galley_pos.y);

                    // First pass: paint the dirty bar for every paragraph
                    // (the constant scaffold from #0023). Y-band lookup is
                    // cheap enough to keep here; we reuse it in the second
                    // pass for the hover icons so the math stays in one
                    // place.
                    let painter = ui.painter().clone();
                    let mut hovered_idx: Option<usize> = None;
                    for (idx, (_id, state, (b_start, b_end))) in
                        paragraph_gutter_marks.iter().enumerate()
                    {
                        let visible_end = b_end.saturating_sub(1).max(*b_start);
                        let c_start = byte_to_char(&app.editor_text, *b_start);
                        let c_end = byte_to_char(&app.editor_text, visible_end);
                        let top_rect = output.galley.pos_from_ccursor(CCursor::new(c_start));
                        let bot_rect = output.galley.pos_from_ccursor(CCursor::new(c_end));
                        let y_top_local = top_rect.top();
                        let y_bot_local = bot_rect.bottom();
                        let y_top = output.galley_pos.y + y_top_local;
                        let y_bot = output.galley_pos.y + y_bot_local;
                        painter.line_segment(
                            [egui::pos2(gutter_x, y_top), egui::pos2(gutter_x, y_bot)],
                            Stroke::new(GUTTER_WIDTH_PX, gutter_color(*state)),
                        );
                        if let Some(py) = pointer_y_local {
                            if py >= y_top_local && py <= y_bot_local && hovered_idx.is_none() {
                                hovered_idx = Some(idx);
                            }
                        }
                    }

                    // Second pass: paint play + trash for the hovered
                    // paragraph only. Anchored at the paragraph's first
                    // line so a long paragraph gets two discreet glyphs at
                    // its top, not stretched along its height.
                    if let Some(idx) = hovered_idx {
                        let (id, _state, (b_start, _b_end)) = &paragraph_gutter_marks[idx];
                        let c_start = byte_to_char(&app.editor_text, *b_start);
                        let top_rect = output.galley.pos_from_ccursor(CCursor::new(c_start));
                        let y_first_top = output.galley_pos.y + top_rect.top();
                        let y_first_bot = output.galley_pos.y + top_rect.bottom();
                        let icon_y = (y_first_top + y_first_bot) * 0.5;
                        let icon_size = egui::vec2(ICON_SIZE_PX + 4.0, ICON_SIZE_PX + 4.0);

                        // Play glyph (#0024).
                        let play_rect = egui::Rect::from_center_size(
                            egui::pos2(play_icon_x, icon_y),
                            icon_size,
                        );
                        let play_resp = ui.interact(
                            play_rect,
                            Id::new(("paragraph-play", id.as_str())),
                            Sense::click(),
                        );
                        let play_color = if play_resp.hovered() {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_MUTED
                        };
                        ui.painter().text(
                            egui::pos2(play_icon_x, icon_y),
                            Align2::CENTER_CENTER,
                            icons::PLAY,
                            FontId::new(ICON_SIZE_PX, family.clone()),
                            play_color,
                        );
                        if play_resp.clicked() {
                            play_clicked = Some(id.clone());
                        }

                        // Trash glyph (#0025): hard-clear all records for
                        // this paragraph. No confirm — the suggestion
                        // store is git-tracked, so a misfire is `git
                        // checkout`-recoverable.
                        let trash_rect = egui::Rect::from_center_size(
                            egui::pos2(trash_icon_x, icon_y),
                            icon_size,
                        );
                        let trash_resp = ui
                            .interact(
                                trash_rect,
                                Id::new(("paragraph-clear", id.as_str())),
                                Sense::click(),
                            )
                            .on_hover_text("Clear all flags for this paragraph");
                        let trash_color = if trash_resp.hovered() {
                            theme::TEXT_PRIMARY
                        } else {
                            theme::TEXT_MUTED
                        };
                        ui.painter().text(
                            egui::pos2(trash_icon_x, icon_y),
                            Align2::CENTER_CENTER,
                            icons::TRASH,
                            FontId::new(ICON_SIZE_PX, family.clone()),
                            trash_color,
                        );
                        if trash_resp.clicked() {
                            clear_clicked = Some(id.clone());
                        }
                    }
                }
                if let Some(id) = play_clicked {
                    app.play_paragraph(&id);
                }
                if let Some(id) = clear_clicked {
                    app.hard_clear_paragraph(&id);
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

                // Right-click → Lock/Unlock paragraph (#0005). Snapshot the
                // target paragraph at click time so the menu closure (which
                // runs every frame the menu is open) reads from a stable
                // value. Galley-relative cursor mapping is the same path
                // hover uses, so wrapping is honoured.
                if response.secondary_clicked() {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let local = pointer - output.galley_pos;
                        if output.galley.rect.contains(local.to_pos2()) {
                            let cursor = output.galley.cursor_from_pos(local);
                            let byte = char_to_byte(&app.editor_text, cursor.ccursor.index);
                            app.editor_context_menu_target = app
                                .current_paragraphs
                                .iter()
                                .find(|p| {
                                    let (s, e) = p.char_range;
                                    byte >= s && byte < e
                                })
                                .map(|p| (p.id.clone(), p.locked));
                        } else {
                            app.editor_context_menu_target = None;
                        }
                    }
                }
                let mut lock_toggle: Option<(String, bool)> = None;
                response.context_menu(|ui| {
                    let Some((pid, currently_locked)) =
                        app.editor_context_menu_target.clone()
                    else {
                        ui.label(
                            RichText::new("right-click on a paragraph")
                                .color(theme::TEXT_MUTED),
                        );
                        return;
                    };
                    let label = if currently_locked {
                        format!("{}  Unlock paragraph", icons::CIRCLE_O)
                    } else {
                        format!("{}  Lock paragraph", icons::CIRCLE)
                    };
                    if ui.button(label).clicked() {
                        lock_toggle = Some((pid, !currently_locked));
                        ui.close_menu();
                    }
                });
                if let Some((pid, new_locked)) = lock_toggle {
                    app.set_paragraph_lock(&pid, new_locked);
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

/// Source of a span's visual contribution. Entities and revisions paint the
/// same `TextFormat` slots (color, underline, background) but with different
/// rules: revisions stack and use the foreground for the underline, entities
/// recolor the glyph and underline themselves at low intensity. LaTeX command
/// tokens (#0015) recolor the glyph but draw no underline; revisions and
/// entities outrank them so a coach flag or character hit on top of `\nl`
/// keeps its usual look.
#[derive(Clone, Copy)]
enum LayerKind {
    Entity,
    Revision,
    LatexCommand,
}

#[derive(Clone, Copy)]
struct Layer {
    start: usize,
    end: usize,
    color: Color32,
    kind: LayerKind,
    /// Higher wins as the sub-span's primary contributor. Selected revision
    /// pins to 255 so a click is always the loudest signal in the region.
    priority: u8,
    selected: bool,
}

/// Background alpha applied to the selected revision's chip color. ~33%
/// reads cleanly on EDITOR_PAGE without obscuring glyphs — the previous
/// flat #332c2c ran at ~7% luminance delta and was effectively invisible.
const SELECTED_TINT_ALPHA: u8 = 0x55;

/// Background alpha applied to the secondary revision in an unselected
/// overlap region. Quiet enough to read as "another flag also lives here"
/// without competing with the primary's underline.
const OVERLAP_TINT_ALPHA: u8 = 0x28;

/// Premultiplied tint of `color` over whatever is behind the glyph. We use
/// alpha rather than pre-blending against `EDITOR_PAGE` so the highlight
/// stays correct if the editor surface ever changes color (egui blends).
fn revision_tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Tokenize the project's three custom LaTeX commands (`\nl`, `\switch`,
/// `\emph{...}`) into pink command spans plus italic byte ranges for
/// `\emph` content (#0015). Linear single-pass scan; no regex.
///
/// Word boundary for `\nl` and `\switch`: end-of-text or next byte not in
/// `[A-Za-z0-9]`. Multi-byte UTF-8 starter bytes (≥0x80) are not ASCII
/// alphanumeric, so a `\nl` followed by `é` still matches. `\emph{` requires
/// a same-paragraph closing `}` (no nesting); a missing brace before `\n` or
/// end-of-text leaves the span un-applied — better dim than fake-emphasized.
fn latex_layers(text: &str) -> (Vec<Layer>, Vec<(usize, usize)>) {
    let mut layers: Vec<Layer> = Vec::new();
    let mut italics: Vec<(usize, usize)> = Vec::new();
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        if i + 3 <= n
            && &bytes[i..i + 3] == b"\\nl"
            && is_latex_word_boundary(bytes, i + 3)
        {
            layers.push(Layer {
                start: i,
                end: i + 3,
                color: theme::LATEX_COMMAND,
                kind: LayerKind::LatexCommand,
                priority: 0,
                selected: false,
            });
            i += 3;
            continue;
        }
        if i + 7 <= n
            && &bytes[i..i + 7] == b"\\switch"
            && is_latex_word_boundary(bytes, i + 7)
        {
            layers.push(Layer {
                start: i,
                end: i + 7,
                color: theme::LATEX_COMMAND,
                kind: LayerKind::LatexCommand,
                priority: 0,
                selected: false,
            });
            i += 7;
            continue;
        }
        if i + 6 <= n && &bytes[i..i + 6] == b"\\emph{" {
            let mut j = i + 6;
            let mut close: Option<usize> = None;
            while j < n {
                match bytes[j] {
                    b'\n' => break,
                    b'}' => {
                        close = Some(j);
                        break;
                    }
                    _ => j += 1,
                }
            }
            if let Some(close) = close {
                layers.push(Layer {
                    start: i,
                    end: i + 6,
                    color: theme::LATEX_COMMAND,
                    kind: LayerKind::LatexCommand,
                    priority: 0,
                    selected: false,
                });
                layers.push(Layer {
                    start: close,
                    end: close + 1,
                    color: theme::LATEX_COMMAND,
                    kind: LayerKind::LatexCommand,
                    priority: 0,
                    selected: false,
                });
                if i + 6 < close {
                    italics.push((i + 6, close));
                }
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    (layers, italics)
}

fn is_latex_word_boundary(bytes: &[u8], pos: usize) -> bool {
    if pos >= bytes.len() {
        return true;
    }
    !bytes[pos].is_ascii_alphanumeric()
}

/// Typography for one editor layout pass. Bundles the four reading knobs
/// (#0020) plus the font family so `build_job` can stay under clippy's
/// `too_many_arguments` threshold.
struct ReadingStyle<'a> {
    font_size: f32,
    line_height: f32,
    letter_spacing: f32,
    family: &'a FontFamily,
}

fn build_job(
    text: &str,
    style: &ReadingStyle<'_>,
    hits: &[EntityHit],
    revisions: &[Revision],
    selected_revision: Option<u32>,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base = TextFormat {
        font_id: FontId::new(style.font_size, style.family.clone()),
        color: theme::TEXT_PRIMARY,
        line_height: Some(style.line_height),
        extra_letter_spacing: style.letter_spacing,
        ..Default::default()
    };

    let text_len = text.len();
    let valid_range = |s: usize, e: usize| {
        s < e
            && e <= text_len
            && text.is_char_boundary(s)
            && text.is_char_boundary(e)
    };

    let mut layers: Vec<Layer> = Vec::new();
    for h in hits {
        if !valid_range(h.start, h.end) {
            continue;
        }
        let color = match h.kind {
            EntityKind::Character => theme::ENTITY_CHARACTER,
            EntityKind::Location => theme::ENTITY_LOCATION,
            _ => theme::TEXT_PRIMARY,
        };
        layers.push(Layer {
            start: h.start,
            end: h.end,
            color,
            kind: LayerKind::Entity,
            // Entity > LatexCommand (50 vs 0) so an entity hit inside an
            // `\emph{...}` paints in the entity color, not pink (#0015).
            priority: 50,
            selected: false,
        });
    }
    for r in revisions {
        let Some((s, e)) = r.anchor else { continue };
        if !valid_range(s, e) {
            continue;
        }
        let selected = selected_revision == Some(r.id);
        // Priority ordering: selected (255) > any revision (100..=103) >
        // entity (0). Within revisions, pipeline_byte gives Spelling > Prose
        // > ShowDontTell > Voice — matches the design notes' "spelling-family
        // > pipeline" precedence in overlap regions.
        let priority = if selected {
            255
        } else {
            100 + pipeline_byte(r.pipeline)
        };
        layers.push(Layer {
            start: s,
            end: e,
            color: revision_color(r),
            kind: LayerKind::Revision,
            priority,
            selected,
        });
    }

    let (latex_cmd_layers, italic_ranges) = latex_layers(text);
    layers.extend(latex_cmd_layers);

    if layers.is_empty() {
        if !text.is_empty() {
            job.append(text, 0.0, base);
        }
        return job;
    }

    // Atomic boundaries: every layer start/end becomes a split point, so
    // each [boundaries[i], boundaries[i+1]) sub-range is covered by a
    // constant set of layers. This is what lets two overlapping revisions
    // both contribute formatting — the previous algorithm dropped any span
    // starting before the running cursor and the second revision vanished.
    // Italic ranges contribute boundaries too so italic is constant per
    // sub-range and composes additively with any layer that wins the slot.
    let mut boundaries: Vec<usize> = layers
        .iter()
        .flat_map(|l| [l.start, l.end])
        .chain(italic_ranges.iter().flat_map(|(s, e)| [*s, *e]))
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();

    let italic_covers = |s: usize, e: usize| {
        italic_ranges
            .iter()
            .any(|(is, ie)| *is <= s && *ie >= e)
    };

    let mut cursor = 0usize;
    for window in boundaries.windows(2) {
        let s = window[0];
        let e = window[1];
        if cursor < s {
            let mut gap = base.clone();
            if italic_covers(cursor, s) {
                gap.italics = true;
            }
            job.append(&text[cursor..s], 0.0, gap);
        }
        let participants: Vec<&Layer> = layers
            .iter()
            .filter(|l| l.start <= s && l.end >= e)
            .collect();
        let mut fmt = base.clone();
        if !participants.is_empty() {
            let primary = participants
                .iter()
                .copied()
                .max_by_key(|l| l.priority)
                .expect("non-empty");
            let revision_layers: Vec<&Layer> = participants
                .iter()
                .copied()
                .filter(|l| matches!(l.kind, LayerKind::Revision))
                .collect();
            let selected_layer = revision_layers.iter().copied().find(|l| l.selected);

            match primary.kind {
                LayerKind::Entity => {
                    fmt.color = primary.color;
                    fmt.underline = Stroke::new(1.0, primary.color.linear_multiply(0.6));
                }
                LayerKind::Revision => {
                    let width = if primary.selected { 3.0 } else { 2.0 };
                    fmt.underline = Stroke::new(width, primary.color);
                }
                LayerKind::LatexCommand => {
                    fmt.color = primary.color;
                }
            }

            if let Some(sel) = selected_layer {
                // Selected anywhere in this sub-range: paint a chip-matching
                // tint so the writer can locate the click target at a glance.
                fmt.background = revision_tint(sel.color, SELECTED_TINT_ALPHA);
            } else if revision_layers.len() >= 2 {
                // Two or more unselected revisions stack here: the primary
                // underlines the region; the secondary's color shows through
                // as a subtle background tint so a click on the dropped card
                // can't read as a no-op.
                let mut sorted = revision_layers.clone();
                sorted.sort_by(|a, b| b.priority.cmp(&a.priority));
                fmt.background = revision_tint(sorted[1].color, OVERLAP_TINT_ALPHA);
            }
        }

        if italic_covers(s, e) {
            fmt.italics = true;
        }

        job.append(&text[s..e], 0.0, fmt);
        cursor = e;
    }
    if cursor < text_len {
        let mut tail = base.clone();
        if italic_covers(cursor, text_len) {
            tail.italics = true;
        }
        job.append(&text[cursor..], 0.0, tail);
    }
    job
}

pub(crate) fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// Convert a byte offset into `text` to its char index. Clamps to the buffer
/// length and walks back to the previous char boundary so out-of-range or
/// mid-codepoint offsets (a stale paragraph range outliving the buffer for a
/// single frame after edit) collapse rather than panic.
fn byte_to_char(text: &str, byte_offset: usize) -> usize {
    let mut clamped = byte_offset.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    text[..clamped].chars().count()
}

/// True when `chars[i..i+3]` is the literal token `\nl` AND it stands alone
/// as a LaTeX command — i.e. the preceding char isn't another backslash
/// (so `\\nl` is rejected) and the following char isn't word-class (so
/// `\nlong`, `\nlabel`, … don't match). Mirrors the `\b` boundary in
/// `book::latex::to_prose`'s drop regex (#0016).
fn is_nl_token_at(chars: &[char], i: usize) -> bool {
    if i + 3 > chars.len() {
        return false;
    }
    if chars[i] != '\\' || chars[i + 1] != 'n' || chars[i + 2] != 'l' {
        return false;
    }
    if i > 0 && chars[i - 1] == '\\' {
        return false;
    }
    if let Some(&c) = chars.get(i + 3) {
        if c.is_ascii_alphanumeric() || c == '_' {
            return false;
        }
    }
    true
}

/// Walk left from `click` to the first char of the paragraph the click is in.
/// Boundaries: a preceding `\nl` token, a blank line (one `\n` followed only
/// by horizontal whitespace then another `\n` or start-of-text), or
/// start-of-text. Bounding token is excluded from the returned range.
fn paragraph_start_char(chars: &[char], click: usize) -> usize {
    let mut i = click.min(chars.len());
    while i > 0 {
        if i >= 3 && is_nl_token_at(chars, i - 3) {
            return i;
        }
        if chars[i - 1] == '\n' {
            let mut j = i - 1;
            while j > 0 && chars[j - 1].is_whitespace() && chars[j - 1] != '\n' {
                j -= 1;
            }
            if j == 0 || chars[j - 1] == '\n' {
                return i;
            }
        }
        i -= 1;
    }
    0
}

/// Walk right from `click` to the position just past the last char of the
/// paragraph. Symmetric to `paragraph_start_char`: stops just before a `\nl`
/// token, before a blank line, or at end-of-text.
fn paragraph_end_char(chars: &[char], click: usize) -> usize {
    let n = chars.len();
    let mut i = click.min(n);
    while i < n {
        if is_nl_token_at(chars, i) {
            return i;
        }
        if chars[i] == '\n' {
            let mut j = i + 1;
            while j < n && chars[j].is_whitespace() && chars[j] != '\n' {
                j += 1;
            }
            if j == n || chars[j] == '\n' {
                return i;
            }
        }
        i += 1;
    }
    n
}

/// Resolve the paragraph that contains `click_char` and return its half-open
/// char range `[start, end)`. Pure char-index logic — independent of the
/// `book::paragraphs` splitter, which works on byte ranges and wraps
/// `\begin{env}` blocks (out of scope for an interactive selection gesture).
/// Returns `(0, 0)` for an empty buffer.
fn paragraph_char_range_at(text: &str, click_char: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let click = click_char.min(chars.len());
    let start = paragraph_start_char(&chars, click);
    let end = paragraph_end_char(&chars, click).max(start);
    (start, end)
}

/// Per-paragraph signal painted in the editor's left margin (#0023). Priority:
/// `Locked` (#0005) wins outright — a hardened paragraph is silenced from every
/// pipeline, and the gutter says so first. Then `HasIssues` overrides the
/// parse-status states, which derive from the show/prose/spelling cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GutterState {
    /// All three per-paragraph pipelines have cached this paragraph at the
    /// current hash, and there are no active issues.
    Clean,
    /// The paragraph has never been seen by any of show/prose/spelling.
    NeverParsed,
    /// At least one of the three pipelines has cached this paragraph, but at
    /// least one cached entry is missing or the hash no longer matches.
    Changed,
    /// At least one non-dismissed revision from show/prose/spelling is anchored
    /// in this paragraph. Wins over the parse-status states because unresolved
    /// feedback is the primary thing the gutter is supposed to surface.
    HasIssues,
    /// The writer has hardened this paragraph (#0005). Outranks every other
    /// state because a locked paragraph is intentionally silent — the
    /// pipelines skip it, so cache freshness and prior issues don't matter.
    Locked,
}

/// Resolve the gutter state for a single paragraph against the chapter's
/// per-pipeline hash cache and the live revision list. Pure so the priority
/// rules can be unit-tested without spinning up the app.
///
/// Voice is excluded from both legs by design: it runs chapter-level, not
/// per-paragraph, so it doesn't have cache entries and its anchored flags
/// don't belong on the per-paragraph signal.
fn gutter_state_for(
    paragraph: &Paragraph,
    last_run_hashes: &BTreeMap<String, BTreeMap<String, String>>,
    revisions: &[Revision],
) -> GutterState {
    if paragraph.locked {
        return GutterState::Locked;
    }
    let has_active_issue = revisions.iter().any(|r| {
        !r.is_dismissed
            && matches!(
                r.pipeline,
                Pipeline::ShowDontTell | Pipeline::Prose | Pipeline::Spelling
            )
            && r.paragraph_id.as_deref() == Some(paragraph.id.as_str())
    });
    if has_active_issue {
        return GutterState::HasIssues;
    }

    let mut any_present = false;
    let mut all_match = true;
    for label in GUTTER_PIPELINE_LABELS {
        match last_run_hashes.get(*label).and_then(|m| m.get(&paragraph.id)) {
            Some(h) => {
                any_present = true;
                if h != &paragraph.hash {
                    all_match = false;
                }
            }
            None => {
                all_match = false;
            }
        }
    }

    if !any_present {
        GutterState::NeverParsed
    } else if all_match {
        GutterState::Clean
    } else {
        GutterState::Changed
    }
}

fn gutter_color(state: GutterState) -> Color32 {
    match state {
        GutterState::Clean => theme::GUTTER_CLEAN,
        GutterState::NeverParsed => theme::GUTTER_NEVER_PARSED,
        GutterState::Changed => theme::GUTTER_CHANGED,
        GutterState::HasIssues => theme::GUTTER_HAS_ISSUES,
        GutterState::Locked => theme::GUTTER_LOCKED,
    }
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
    letter_spacing: f32,
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
    h.update(&inp.letter_spacing.to_bits().to_le_bytes());
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
        letter_spacing: f32,
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
                letter_spacing: 0.4,
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
                letter_spacing: self.letter_spacing,
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
            letter_spacing: 0.4,
            family_label: "",
            wrap_width: 600.0,
        };
        assert_eq!(layout_fingerprint(&inp), layout_fingerprint(&inp));
    }

    // --- Dirty gutter (#0023) ----------------------------------------------

    use crate::book::paragraphs::parse_and_match;

    fn long_para(seed: &str) -> String {
        format!("{seed} this is a long paragraph with plenty of text to support the splitter and avoid the short-paragraph fallback path entirely.")
    }

    /// Build a chapter with three long paragraphs and return the parsed
    /// paragraph index. Hashes are deterministic across calls so tests can
    /// populate the cache straight from the parsed list.
    fn three_para_chapter() -> Vec<Paragraph> {
        let p_a = long_para("Alpha");
        let p_b = long_para("Beta");
        let p_c = long_para("Gamma");
        let text = format!("{p_a}\n\n{p_b}\n\n{p_c}\n");
        parse_and_match(&text, &[])
    }

    fn full_cache_for(paragraphs: &[Paragraph]) -> BTreeMap<String, String> {
        paragraphs
            .iter()
            .map(|p| (p.id.clone(), p.hash.clone()))
            .collect()
    }

    fn three_label_cache(
        paragraphs: &[Paragraph],
    ) -> BTreeMap<String, BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        let cached = full_cache_for(paragraphs);
        out.insert("show, don't tell".into(), cached.clone());
        out.insert("prose".into(), cached.clone());
        out.insert("spelling".into(), cached);
        out
    }

    fn revision_anchored_in(
        id: u32,
        pipeline: Pipeline,
        paragraph_id: &str,
        is_dismissed: bool,
    ) -> Revision {
        Revision {
            id,
            pipeline,
            kind: FlagKind::Other,
            quote: "q".to_string(),
            why: "w".to_string(),
            suggestion: "s".to_string(),
            anchor: Some((0, 1)),
            suggestion_id: format!("sid-{id}"),
            paragraph_id: Some(paragraph_id.to_string()),
            is_dismissed,
        }
    }

    fn states_for(
        paragraphs: &[Paragraph],
        cache: &BTreeMap<String, BTreeMap<String, String>>,
        revisions: &[Revision],
    ) -> Vec<GutterState> {
        paragraphs
            .iter()
            .map(|p| gutter_state_for(p, cache, revisions))
            .collect()
    }

    #[test]
    fn empty_cache_marks_every_paragraph_never_parsed() {
        let paragraphs = three_para_chapter();
        let states = states_for(&paragraphs, &BTreeMap::new(), &[]);
        assert!(
            states.iter().all(|s| *s == GutterState::NeverParsed),
            "fresh chapter should be all NeverParsed: {states:?}"
        );
    }

    #[test]
    fn full_cache_across_three_pipelines_marks_clean() {
        let paragraphs = three_para_chapter();
        let cache = three_label_cache(&paragraphs);
        let states = states_for(&paragraphs, &cache, &[]);
        assert!(
            states.iter().all(|s| *s == GutterState::Clean),
            "fully cached chapter should be all Clean: {states:?}"
        );
    }

    #[test]
    fn partial_pipeline_coverage_marks_changed_not_never_parsed() {
        // Only prose has been run. show/spelling caches are absent — so the
        // paragraphs *have* been parsed (by one pipeline) but not by the
        // others. They should read as Changed, not NeverParsed: a coach run
        // happened, but the union isn't satisfied.
        let paragraphs = three_para_chapter();
        let mut cache: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        cache.insert("prose".into(), full_cache_for(&paragraphs));
        let states = states_for(&paragraphs, &cache, &[]);
        assert!(
            states.iter().all(|s| *s == GutterState::Changed),
            "partial cache should be all Changed: {states:?}"
        );
    }

    #[test]
    fn voice_cache_does_not_satisfy_any_state() {
        // Voice runs chapter-level; populating its label shouldn't shift any
        // paragraph off NeverParsed.
        let paragraphs = three_para_chapter();
        let mut cache: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        cache.insert("voice".into(), full_cache_for(&paragraphs));
        let states = states_for(&paragraphs, &cache, &[]);
        assert!(
            states.iter().all(|s| *s == GutterState::NeverParsed),
            "voice-only cache should still be NeverParsed: {states:?}"
        );
    }

    #[test]
    fn stale_hash_in_one_pipeline_marks_changed() {
        let paragraphs = three_para_chapter();
        let mut cache = three_label_cache(&paragraphs);
        // Corrupt prose's cached hash for the middle paragraph: simulates the
        // writer editing+saving the paragraph since the last prose run.
        let mid_id = paragraphs[1].id.clone();
        cache
            .get_mut("prose")
            .expect("prose label seeded")
            .insert(mid_id, "deadbeefdeadbeef".to_string());
        let states = states_for(&paragraphs, &cache, &[]);
        assert_eq!(states[0], GutterState::Clean);
        assert_eq!(states[1], GutterState::Changed);
        assert_eq!(states[2], GutterState::Clean);
    }

    #[test]
    fn one_paragraph_missing_in_one_pipeline_marks_changed() {
        let paragraphs = three_para_chapter();
        let mut cache = three_label_cache(&paragraphs);
        let mid_id = paragraphs[1].id.clone();
        cache.get_mut("spelling").expect("spelling seeded").remove(&mid_id);
        let states = states_for(&paragraphs, &cache, &[]);
        assert_eq!(states[0], GutterState::Clean);
        assert_eq!(states[1], GutterState::Changed);
        assert_eq!(states[2], GutterState::Clean);
    }

    #[test]
    fn active_issue_overrides_clean() {
        let paragraphs = three_para_chapter();
        let cache = three_label_cache(&paragraphs);
        let mid_id = paragraphs[1].id.clone();
        let revs = vec![revision_anchored_in(1, Pipeline::Prose, &mid_id, false)];
        let states = states_for(&paragraphs, &cache, &revs);
        assert_eq!(states[0], GutterState::Clean);
        assert_eq!(states[1], GutterState::HasIssues);
        assert_eq!(states[2], GutterState::Clean);
    }

    #[test]
    fn active_issue_overrides_changed_and_never_parsed() {
        // HasIssues is the highest-priority state; a paragraph with an active
        // revision is red regardless of its parse status.
        let paragraphs = three_para_chapter();
        let mid_id = paragraphs[1].id.clone();
        let revs = vec![revision_anchored_in(1, Pipeline::Spelling, &mid_id, false)];
        // Empty cache → would otherwise be NeverParsed.
        let states = states_for(&paragraphs, &BTreeMap::new(), &revs);
        assert_eq!(states[1], GutterState::HasIssues);
    }

    #[test]
    fn dismissed_issue_does_not_override() {
        let paragraphs = three_para_chapter();
        let cache = three_label_cache(&paragraphs);
        let mid_id = paragraphs[1].id.clone();
        let revs = vec![revision_anchored_in(1, Pipeline::Prose, &mid_id, true)];
        let states = states_for(&paragraphs, &cache, &revs);
        assert!(
            states.iter().all(|s| *s == GutterState::Clean),
            "dismissed-only revision should not push to HasIssues: {states:?}"
        );
    }

    #[test]
    fn voice_revision_does_not_override() {
        // Voice issues anchor in paragraphs but they're chapter-level
        // semantically; the gutter ignores them.
        let paragraphs = three_para_chapter();
        let cache = three_label_cache(&paragraphs);
        let mid_id = paragraphs[1].id.clone();
        let revs = vec![revision_anchored_in(1, Pipeline::Voice, &mid_id, false)];
        let states = states_for(&paragraphs, &cache, &revs);
        assert!(
            states.iter().all(|s| *s == GutterState::Clean),
            "voice revision should not push to HasIssues: {states:?}"
        );
    }

    #[test]
    fn locked_paragraph_overrides_has_issues() {
        // Locked is the highest-priority gutter state — even an active
        // unresolved issue must not push past it. Lock is the writer's
        // explicit "stop telling me about this paragraph."
        let mut paragraphs = three_para_chapter();
        let cache = three_label_cache(&paragraphs);
        let mid_id = paragraphs[1].id.clone();
        paragraphs[1].locked = true;
        let revs = vec![revision_anchored_in(1, Pipeline::Prose, &mid_id, false)];
        let states = states_for(&paragraphs, &cache, &revs);
        assert_eq!(states[1], GutterState::Locked);
    }

    #[test]
    fn locked_paragraph_overrides_clean() {
        // Sanity — locked also wins against the quiet baseline state, so
        // the writer can spot which paragraphs they've hardened at a
        // glance against the otherwise-mostly-Clean column.
        let mut paragraphs = three_para_chapter();
        let cache = three_label_cache(&paragraphs);
        paragraphs[0].locked = true;
        let states = states_for(&paragraphs, &cache, &[]);
        assert_eq!(states[0], GutterState::Locked);
        assert_eq!(states[1], GutterState::Clean);
        assert_eq!(states[2], GutterState::Clean);
    }

    #[test]
    fn unrelated_paragraph_id_does_not_propagate_issue() {
        let paragraphs = three_para_chapter();
        let cache = three_label_cache(&paragraphs);
        // A revision anchored to a paragraph_id that doesn't match any current
        // paragraph (e.g. a stale record from before a paragraph was deleted).
        let revs = vec![revision_anchored_in(
            1,
            Pipeline::Prose,
            "p_deadbeef",
            false,
        )];
        let states = states_for(&paragraphs, &cache, &revs);
        assert!(
            states.iter().all(|s| *s == GutterState::Clean),
            "stray revision must not turn any paragraph red: {states:?}"
        );
    }

    // --- byte_to_char ------------------------------------------------------

    #[test]
    fn byte_to_char_maps_ascii_one_to_one() {
        assert_eq!(byte_to_char("hello", 0), 0);
        assert_eq!(byte_to_char("hello", 3), 3);
        assert_eq!(byte_to_char("hello", 5), 5);
    }

    #[test]
    fn byte_to_char_clamps_past_eof() {
        assert_eq!(byte_to_char("hi", 99), 2);
    }

    #[test]
    fn byte_to_char_handles_multibyte_codepoints() {
        // "é" is 2 bytes in UTF-8.
        let s = "café";
        assert_eq!(byte_to_char(s, 0), 0);
        assert_eq!(byte_to_char(s, 3), 3); // start of "é"
        assert_eq!(byte_to_char(s, 5), 4); // after "é"
        // Mid-codepoint byte (4) snaps back to the previous boundary (3).
        assert_eq!(byte_to_char(s, 4), 3);
    }

    // --- build_job overlap + selected indicator (#0026) -------------------

    fn coach_revision(id: u32, pipeline: Pipeline, anchor: (usize, usize)) -> Revision {
        Revision {
            id,
            pipeline,
            kind: match pipeline {
                Pipeline::Spelling => FlagKind::Spelling,
                _ => FlagKind::Other,
            },
            quote: String::new(),
            why: String::new(),
            suggestion: String::new(),
            anchor: Some(anchor),
            suggestion_id: format!("sid-{id}"),
            paragraph_id: None,
            is_dismissed: false,
        }
    }

    fn run_build_job(text: &str, revisions: &[Revision], selected: Option<u32>) -> LayoutJob {
        run_build_job_with_hits(text, &[], revisions, selected)
    }

    fn run_build_job_with_hits(
        text: &str,
        hits: &[EntityHit],
        revisions: &[Revision],
        selected: Option<u32>,
    ) -> LayoutJob {
        let style = ReadingStyle {
            font_size: 18.0,
            line_height: 30.0,
            letter_spacing: 0.4,
            family: &FontFamily::Proportional,
        };
        build_job(text, &style, hits, revisions, selected)
    }

    #[test]
    fn overlapping_revisions_both_contribute_formatting() {
        // Regression: the previous algorithm walked spans sorted by start
        // and skipped any whose start fell before the running cursor, which
        // silently dropped the second flag in any overlap. Both
        // revisions' colors must now appear somewhere in the LayoutJob's
        // section list — a click on the dropped card no longer reads as a
        // no-op.
        let text = "the quick brown fox jumps over the lazy dog and runs.";
        let prose = coach_revision(1, Pipeline::Prose, (4, 19)); // "quick brown fox"
        let spelling = coach_revision(2, Pipeline::Spelling, (10, 25)); // "brown fox jumps"

        let job = run_build_job(text, &[prose, spelling], None);

        let prose_color = theme::REVISION_PROSE;
        let spelling_color = theme::REVISION_SPELLING;
        let prose_present = job
            .sections
            .iter()
            .any(|s| s.format.underline.color == prose_color);
        let spelling_present = job
            .sections
            .iter()
            .any(|s| s.format.underline.color == spelling_color);
        assert!(
            prose_present,
            "prose revision must contribute formatting: {:#?}",
            job.sections
        );
        assert!(
            spelling_present,
            "spelling revision must contribute formatting: {:#?}",
            job.sections
        );
    }

    #[test]
    fn coincident_unselected_overlap_keeps_secondary_color_visible() {
        // When two revisions share the exact same anchor with no selection,
        // there's only one sub-range to format. The lower-priority flag
        // still gets a visual hand-off via a tinted background — without
        // it, the dropped card would feel dead even after the overlap fix.
        let text = "the quick brown fox";
        let show = coach_revision(1, Pipeline::ShowDontTell, (4, 9));
        let spelling = coach_revision(2, Pipeline::Spelling, (4, 9));

        let job = run_build_job(text, &[show, spelling], None);

        let overlap = job
            .sections
            .iter()
            .find(|s| s.byte_range.start == 4 && s.byte_range.end == 9)
            .expect("overlap section");
        // Spelling has the higher pipeline_byte, so it wins the underline.
        assert_eq!(overlap.format.underline.color, theme::REVISION_SPELLING);
        // Show's color shows through as the background tint — exact value
        // pinned so we catch accidental regressions of the alpha constant.
        assert_eq!(
            overlap.format.background,
            revision_tint(theme::REVISION_SHOW, OVERLAP_TINT_ALPHA),
        );
    }

    #[test]
    fn selected_revision_paints_chip_tinted_background() {
        // The previous code painted REVISION_SELECTED_BG (#332c2c) for the
        // selected span — ~7% luminance delta against EDITOR_PAGE, visually
        // a non-event. The new behaviour tints with the revision's own
        // color so the highlight matches the card chip the writer clicked.
        let text = "the quick brown fox";
        let r = coach_revision(7, Pipeline::Prose, (4, 9));

        let job = run_build_job(text, &[r], Some(7));

        let section = job
            .sections
            .iter()
            .find(|s| s.byte_range.start == 4 && s.byte_range.end == 9)
            .expect("selected section");
        assert_eq!(
            section.format.background,
            revision_tint(theme::REVISION_PROSE, SELECTED_TINT_ALPHA),
        );
        // Selected underline thickens to 3px so short spans stay legible.
        assert!(
            (section.format.underline.width - 3.0).abs() < f32::EPSILON,
            "selected underline must be 3px, got {}",
            section.format.underline.width
        );
        assert_eq!(section.format.underline.color, theme::REVISION_PROSE);
    }

    // --- LaTeX command highlighting (#0015) -------------------------------

    fn section_for(job: &LayoutJob, range: (usize, usize)) -> &egui::text::LayoutSection {
        job.sections
            .iter()
            .find(|s| s.byte_range.start == range.0 && s.byte_range.end == range.1)
            .unwrap_or_else(|| {
                panic!(
                    "no section for byte range {:?}: {:#?}",
                    range, job.sections
                )
            })
    }

    #[test]
    fn nl_command_renders_in_pink() {
        // Canary for "did the slash actually land": `\nl` must color all four
        // characters so a typo (`\n1`) reads as plain prose by contrast.
        let text = "first line\\nl second line";
        let job = run_build_job(text, &[], None);
        let section = section_for(&job, (10, 13));
        assert_eq!(section.format.color, theme::LATEX_COMMAND);
    }

    #[test]
    fn switch_command_renders_in_pink() {
        let text = "scene one\\switch scene two";
        let job = run_build_job(text, &[], None);
        let section = section_for(&job, (9, 16));
        assert_eq!(section.format.color, theme::LATEX_COMMAND);
    }

    #[test]
    fn emph_braces_pink_and_content_italic() {
        // `\emph{` and `}` get the command color; the content between renders
        // italic in default text color so the editor mirrors the PDF.
        let text = "say \\emph{hello} now";
        let job = run_build_job(text, &[], None);

        let open = section_for(&job, (4, 10)); // `\emph{`
        assert_eq!(open.format.color, theme::LATEX_COMMAND);
        assert!(!open.format.italics);

        let body = section_for(&job, (10, 15)); // `hello`
        assert_eq!(body.format.color, theme::TEXT_PRIMARY);
        assert!(body.format.italics);

        let close = section_for(&job, (15, 16)); // `}`
        assert_eq!(close.format.color, theme::LATEX_COMMAND);
        assert!(!close.format.italics);
    }

    #[test]
    fn near_miss_typos_do_not_highlight() {
        // `\n1` (digit) and `\Switch` (capital) are the canary cases the
        // ticket calls out — both must stay default-colored so the writer
        // can spot them by contrast against a real `\nl` / `\switch`.
        let text = "ok \\n1 and \\Switch end";
        let job = run_build_job(text, &[], None);
        for section in &job.sections {
            assert_ne!(
                section.format.color,
                theme::LATEX_COMMAND,
                "near-miss typo must not highlight: {:#?}",
                section
            );
            assert!(!section.format.italics);
        }
    }

    #[test]
    fn unmatched_emph_brace_leaves_span_unapplied() {
        // No closing `}` before end-of-paragraph: the design says leave the
        // span dim rather than coloring to end-of-text. Better an obvious
        // miss than a runaway pink tail.
        let text = "say \\emph{hello and never close";
        let job = run_build_job(text, &[], None);
        for section in &job.sections {
            assert_ne!(section.format.color, theme::LATEX_COMMAND);
            assert!(!section.format.italics);
        }
    }

    #[test]
    fn emph_brace_does_not_cross_paragraph_boundary() {
        // `\n` inside the buffer terminates the search — `\emph{` on one
        // line with the `}` on the next line is treated as unmatched.
        let text = "say \\emph{hello\nworld} end";
        let job = run_build_job(text, &[], None);
        for section in &job.sections {
            assert_ne!(section.format.color, theme::LATEX_COMMAND);
            assert!(!section.format.italics);
        }
    }

    #[test]
    fn entity_inside_emph_keeps_entity_color_and_italic() {
        // `\emph{Skari}` with Skari matched as a Character entity: the
        // entity color outranks the LaTeX command color (priority 50 vs 0)
        // and italic applies additively from the brace range.
        let text = "and \\emph{Skari} arrives";
        let hit = EntityHit {
            start: 10,
            end: 15, // "Skari"
            entity_id: "skari".to_string(),
            kind: EntityKind::Character,
        };
        let job = run_build_job_with_hits(text, &[hit], &[], None);

        let body = section_for(&job, (10, 15));
        assert_eq!(body.format.color, theme::ENTITY_CHARACTER);
        assert!(body.format.italics);

        let open = section_for(&job, (4, 10));
        assert_eq!(open.format.color, theme::LATEX_COMMAND);
        let close = section_for(&job, (15, 16));
        assert_eq!(close.format.color, theme::LATEX_COMMAND);
    }

    #[test]
    fn revision_underline_survives_over_nl() {
        // A revision anchored over `\nl` must still show its underline —
        // revisions outrank LaTeX command color, the underline is the
        // primary visual signal regardless of what's beneath it.
        let text = "first\\nl second";
        let r = coach_revision(1, Pipeline::Prose, (5, 8)); // "\nl"
        let job = run_build_job(text, &[r], None);
        let section = section_for(&job, (5, 8));
        assert_eq!(section.format.underline.color, theme::REVISION_PROSE);
    }

    // --- Multi-click paragraph selection (#0016) --------------------------
    //
    // The 1/2/3-click steps are owned by upstream egui (cursor / word /
    // line). We only test the 4-click paragraph step's range computation,
    // because that's the only logic this crate adds. Click-counter cadence
    // (400 ms / 3 px) is wired in the editor closure and exercised by hand
    // — there's no headless egui harness here to drive pointer events.

    /// Click anywhere in a single paragraph and the selection covers the
    /// whole paragraph, excluding the trailing `\n`.
    #[test]
    fn paragraph_range_single_paragraph() {
        let text = "the cat sat on the mat";
        // Click somewhere in the middle.
        let r = paragraph_char_range_at(text, 8);
        assert_eq!(r, (0, text.chars().count()));
    }

    #[test]
    fn paragraph_range_picks_paragraph_under_click() {
        // Three paragraphs separated by blank lines. A click in the middle
        // paragraph must select only the middle paragraph, neither sibling.
        let text = "first paragraph.\n\nsecond paragraph.\n\nthird paragraph.";
        // Click in "second" — find its char index.
        let click = text.find("second").unwrap();
        let (start, end) = paragraph_char_range_at(text, click + 1);
        assert_eq!(&text[start..end], "second paragraph.");
    }

    #[test]
    fn paragraph_range_blank_line_with_horizontal_whitespace() {
        // A "blank line" can contain spaces and tabs — the splitter must
        // still treat it as a paragraph break.
        let text = "first.\n   \t  \nsecond.";
        let click = text.find("second").unwrap();
        let (start, end) = paragraph_char_range_at(text, click);
        assert_eq!(&text[start..end], "second.");
    }

    #[test]
    fn paragraph_range_excludes_trailing_newline() {
        // The selection must stop at the `\n` of the paragraph, not include
        // it — so a 4-click followed by Delete erases the paragraph but
        // leaves the surrounding blank-line scaffold intact.
        let text = "alpha\nbravo\n\ncharlie";
        let (start, end) = paragraph_char_range_at(text, 0);
        assert_eq!(&text[start..end], "alpha\nbravo");
    }

    #[test]
    fn paragraph_range_clicks_before_nl_stop_at_nl() {
        // `\nl` mid-paragraph is itself a paragraph boundary. A click on
        // the prose before `\nl` selects only up to the token; the token
        // and what follows belong to a separate paragraph. The token
        // requires a word boundary on the right (matches the rest of the
        // codebase's `\\nl\b` rule), hence the space after it.
        let text = "alpha part. \\nl beta part.";
        let click = text.find("alpha").unwrap() + 1;
        let (start, end) = paragraph_char_range_at(text, click);
        assert_eq!(&text[start..end], "alpha part. ");
    }

    #[test]
    fn paragraph_range_clicks_after_nl_start_after_nl() {
        let text = "alpha part. \\nl beta part.";
        let click = text.find("beta").unwrap() + 1;
        let (start, end) = paragraph_char_range_at(text, click);
        assert_eq!(&text[start..end], " beta part.");
    }

    #[test]
    fn paragraph_range_nl_at_start_or_end_of_buffer() {
        let text = "\\nl only paragraph. \\nl";
        let click = text.find("only").unwrap();
        let (start, end) = paragraph_char_range_at(text, click);
        assert_eq!(&text[start..end], " only paragraph. ");
    }

    #[test]
    fn paragraph_range_ignores_word_extension_after_nl() {
        // `\nlong` is a different command — it must NOT be treated as a
        // paragraph break. Selection should span the whole buffer.
        let text = "before \\nlong continuation here.";
        let click = text.find("continuation").unwrap();
        let (start, end) = paragraph_char_range_at(text, click);
        assert_eq!(&text[start..end], text);
    }

    #[test]
    fn paragraph_range_ignores_escaped_double_backslash_nl() {
        // `\\nl` is `\\` (line break) followed by literal `nl` — not the
        // `\nl` paragraph token. A click after it must still see one
        // contiguous paragraph.
        let text = "before \\\\nl after";
        let click = text.find("after").unwrap();
        let (start, end) = paragraph_char_range_at(text, click);
        assert_eq!(&text[start..end], text);
    }

    #[test]
    fn paragraph_range_empty_buffer_yields_empty_range() {
        assert_eq!(paragraph_char_range_at("", 0), (0, 0));
    }

    #[test]
    fn paragraph_range_clamps_overshooting_click() {
        // A stale click index past the buffer must collapse to end-of-text
        // rather than panic.
        let text = "single paragraph";
        let (start, end) = paragraph_char_range_at(text, 999);
        assert_eq!((start, end), (0, text.chars().count()));
    }

    #[test]
    fn paragraph_range_handles_multibyte_chars() {
        // The helper indexes by char, not byte. A buffer with a
        // multi-byte codepoint must still split on `\nl` correctly and
        // return char indices.
        let text = "café and tea \\nl rest of buffer";
        let click_chars: usize = text[..text.find("café").unwrap() + 1].chars().count();
        let (start, end) = paragraph_char_range_at(text, click_chars);
        let chars: Vec<char> = text.chars().collect();
        let selected: String = chars[start..end].iter().collect();
        assert_eq!(selected, "café and tea ");
    }

    // --- \nl-token detection edge cases -----------------------------------

    #[test]
    fn nl_token_recognises_bare_token() {
        let chars: Vec<char> = r"\nl".chars().collect();
        assert!(is_nl_token_at(&chars, 0));
    }

    #[test]
    fn nl_token_rejects_word_extension() {
        let chars: Vec<char> = r"\nlong".chars().collect();
        assert!(!is_nl_token_at(&chars, 0));
    }

    #[test]
    fn nl_token_rejects_escaped_backslash() {
        let chars: Vec<char> = r"\\nl".chars().collect();
        assert!(!is_nl_token_at(&chars, 1));
    }

    #[test]
    fn nl_token_accepts_followed_by_punctuation() {
        let chars: Vec<char> = r"\nl{".chars().collect();
        assert!(is_nl_token_at(&chars, 0));
    }
}
