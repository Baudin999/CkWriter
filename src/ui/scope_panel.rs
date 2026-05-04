use crate::app::CkWriterApp;
use crate::book::entity::{Entity, EntityKind};
use crate::extract;
use crate::llm::characters::{ProposalStatus, ProposalVerdict};
use crate::theme;
use egui::{Color32, RichText};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Characters,
    Locations,
    AI,
    Chat,
    Chapter,
}

/// Sub-tabs inside the Characters tab. Each is its own scrollable view so
/// long lists never break the panel layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharSubTab {
    Personae,
    Cast,
    Proposals,
    AiOutput,
}

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for (tab, label) in [
            (Tab::Characters, "Characters"),
            (Tab::Locations, "Locations"),
            (Tab::AI, "AI"),
            (Tab::Chat, "Chat"),
            (Tab::Chapter, "Chapter"),
        ] {
            let selected = app.scope_tab == tab;
            if ui.selectable_label(selected, label).clicked() {
                app.scope_tab = tab;
            }
        }
    });
    ui.separator();

    match app.scope_tab {
        Tab::Characters => show_characters(app, ui),
        Tab::Locations => show_locations(app, ui),
        Tab::AI => show_ai(app, ui),
        Tab::Chat => show_chat(app, ui),
        Tab::Chapter => show_chapter(app, ui),
    }
}

// ─────────────────────────── Characters ────────────────────────────

fn show_characters(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.book.is_none() {
        return;
    }

    actions_row(app, ui);
    extraction_status(app, ui);
    ui.add_space(4.0);
    sub_tabs_row(app, ui);
    ui.add_space(4.0);

    // Cast & Personae render their own master/detail with internal scroll
    // areas and need full horizontal width — so we don't wrap them in an
    // outer ScrollArea. Proposals & AI Output keep the simple list layout.
    match app.char_sub_tab {
        CharSubTab::Personae => personae_tab(app, ui),
        CharSubTab::Cast => cast_tab(app, ui),
        CharSubTab::Proposals => {
            egui::ScrollArea::vertical()
                .id_salt("char-proposals-scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| proposals_tab(app, ui));
        }
        CharSubTab::AiOutput => {
            egui::ScrollArea::vertical()
                .id_salt("char-ai-output-scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| ai_output_tab(app, ui));
        }
    }
}

fn actions_row(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    let busy = app.char_stream.is_some();
    let can_extract = !busy && app.current_chapter.is_some() && app.ollama_ok;
    let none_yet = app
        .book
        .as_ref()
        .map(|b| b.entities_of(EntityKind::Character).is_empty())
        .unwrap_or(true);

    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(can_extract, |ui| {
            if ui.button("Find with AI").clicked() {
                app.extract_characters_from_chapter();
            }
        });
        if none_yet && ui.button("Import Personae.txt").clicked() {
            app.run_import();
        }
        if ui.button("+ Add").clicked() {
            app.create_blank_entity(EntityKind::Character);
        }
    });
}

fn extraction_status(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.char_stream.is_some() {
        ui.label(
            RichText::new("● extracting…")
                .small()
                .color(theme::REVISION_VOICE),
        );
    } else if let Some(err) = &app.char_extract_error {
        ui.label(RichText::new(err).small().color(Color32::LIGHT_RED));
    } else if !app.ollama_ok {
        ui.label(
            RichText::new("ollama unreachable")
                .small()
                .color(Color32::LIGHT_RED),
        );
    } else if app.current_chapter.is_none() {
        ui.label(
            RichText::new("open a chapter to extract")
                .small()
                .color(theme::TEXT_MUTED),
        );
    } else if let Some(s) = &app.import_status {
        ui.label(RichText::new(s).small().color(theme::TEXT_MUTED));
    }
}

fn sub_tabs_row(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    let personae_count = app
        .book
        .as_ref()
        .map(|b| b.entities_of(EntityKind::Character).len())
        .unwrap_or(0);
    let cast_count = extract::by_kind(
        &extract::frequency_map(&app.entity_hits),
        EntityKind::Character,
    )
    .len();
    let new_count = app
        .char_proposals
        .iter()
        .filter(|p| {
            p.status == ProposalStatus::Pending && matches!(p.verdict, ProposalVerdict::New)
        })
        .count();
    let pending_count = app
        .char_proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Pending)
        .count();

    ui.horizontal_wrapped(|ui| {
        sub_tab_button(
            ui,
            &mut app.char_sub_tab,
            CharSubTab::Cast,
            "Cast",
            count_suffix(cast_count),
            None,
        );
        sub_tab_button(
            ui,
            &mut app.char_sub_tab,
            CharSubTab::Personae,
            "Personae",
            count_suffix(personae_count),
            None,
        );
        let badge = if new_count > 0 {
            Some((format!("{new_count} new"), theme::ACCENT))
        } else if pending_count > 0 {
            Some((format!("{pending_count}"), theme::TEXT_MUTED))
        } else {
            None
        };
        sub_tab_button(
            ui,
            &mut app.char_sub_tab,
            CharSubTab::Proposals,
            "Proposals",
            String::new(),
            badge,
        );
        sub_tab_button(
            ui,
            &mut app.char_sub_tab,
            CharSubTab::AiOutput,
            "AI Output",
            String::new(),
            None,
        );
    });
}

fn sub_tab_button(
    ui: &mut egui::Ui,
    state: &mut CharSubTab,
    target: CharSubTab,
    label: &str,
    count_suffix: String,
    badge: Option<(String, Color32)>,
) {
    let selected = *state == target;
    let mut text = RichText::new(format!("{label}{count_suffix}")).strong();
    if !selected {
        text = text.color(theme::TEXT_MUTED);
    }
    let mut clicked = ui.selectable_label(selected, text).clicked();
    if let Some((badge_text, color)) = badge {
        let resp = ui.label(
            RichText::new(badge_text)
                .small()
                .color(Color32::BLACK)
                .background_color(color),
        );
        clicked |= resp.clicked();
    }
    if clicked {
        *state = target;
    }
}

fn count_suffix(n: usize) -> String {
    if n == 0 {
        String::new()
    } else {
        format!(" ({n})")
    }
}

// ─────────────────────────── Master / detail shared ───────────────

/// One master-list row entry. `count` is set for Cast (chapter-frequency),
/// `None` for Personae.
struct MasterRow {
    id: String,
    name: String,
    aliases: Vec<String>,
    category: String,
    count: Option<usize>,
}

fn name_matches(query: &str, name: &str, aliases: &[String]) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    if name.to_lowercase().contains(&q) {
        return true;
    }
    aliases.iter().any(|a| a.to_lowercase().contains(&q))
}

fn search_input(ui: &mut egui::Ui, query: &mut String) {
    ui.add(
        egui::TextEdit::singleline(query)
            .hint_text("search…")
            .desired_width(f32::INFINITY),
    );
}

fn master_row(ui: &mut egui::Ui, selected: bool, row: &MasterRow) -> bool {
    let bg = if selected {
        theme::BG_INSET
    } else {
        Color32::TRANSPARENT
    };
    let resp = egui::Frame::group(ui.style())
        .fill(bg)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            // Always claim the full master-column width so cards line up
            // regardless of whether the row has trailing right-aligned content.
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&row.name)
                        .color(theme::ENTITY_CHARACTER)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(c) = row.count {
                        ui.label(
                            RichText::new(format!("\u{00D7}{c}"))
                                .small()
                                .color(theme::TEXT_MUTED),
                        );
                    }
                });
            });
            if !row.aliases.is_empty() {
                ui.label(
                    RichText::new(row.aliases.join(", "))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            if !row.category.is_empty() {
                ui.label(
                    RichText::new(&row.category)
                        .small()
                        .color(theme::TEXT_MUTED)
                        .italics(),
                );
            }
        })
        .response
        .interact(egui::Sense::click());
    resp.clicked()
}

/// Render the right-hand detail column for the currently-selected entity.
/// `entity` is pre-cloned so the inner closure doesn't have to re-borrow
/// app.book while app is also borrowed mutably for inspector edits.
fn detail_column(app: &mut CkWriterApp, ui: &mut egui::Ui, entity: Option<&Entity>) {
    egui::ScrollArea::vertical()
        .id_salt("master-detail-scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| match entity {
            Some(e) => crate::ui::inspector::render_detail(app, ui, e),
            None => empty_state(
                ui,
                "Pick a character",
                "Click someone in the list to see their details.",
            ),
        });
}

fn master_list_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() * 0.38).clamp(180.0, 280.0)
}

// ─────────────────────────── Personae sub-tab ──────────────────────

fn personae_tab(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    // Gather data first (immutable book read), so the layout closures can
    // re-borrow `app` mutably one at a time without nested-borrow conflicts.
    let Some(book) = app.book.as_ref() else {
        return;
    };
    let mut rows: Vec<MasterRow> = book
        .entities_of(EntityKind::Character)
        .iter()
        .map(|e| MasterRow {
            id: e.id.clone(),
            name: e.name.clone(),
            aliases: e.aliases.clone(),
            category: e.category.clone(),
            count: None,
        })
        .collect();

    if rows.is_empty() {
        empty_state(
            ui,
            "No characters yet.",
            "Run Find with AI on a chapter, or import from Personae.txt.",
        );
        return;
    }

    let category_order: Vec<String> = book.data.categories.clone();
    let any_categorised = rows.iter().any(|r| !r.category.is_empty());

    let query = app.character_search.clone();
    if !query.is_empty() {
        rows.retain(|r| name_matches(&query, &r.name, &r.aliases));
    }

    let selected_id = app.selected_entity.clone();
    let detail_entity: Option<Entity> =
        selected_id.as_ref().and_then(|id| book.entity(id).cloned());

    master_detail_layout(
        app,
        ui,
        |ui, app| {
            // Master column
            search_input(ui, &mut app.character_search);
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("personae-list-scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if rows.is_empty() {
                        ui.label(RichText::new("no matches").small().color(theme::TEXT_MUTED));
                        return;
                    }
                    let group = any_categorised && query.is_empty();
                    if group {
                        let buckets: Vec<String> = category_order
                            .iter()
                            .cloned()
                            .chain(std::iter::once("Uncategorised".to_string()))
                            .collect();
                        for cat in &buckets {
                            let in_bucket: Vec<&MasterRow> = rows
                                .iter()
                                .filter(|r| {
                                    if cat == "Uncategorised" {
                                        r.category.is_empty()
                                    } else {
                                        &r.category == cat
                                    }
                                })
                                .collect();
                            if in_bucket.is_empty() {
                                continue;
                            }
                            ui.label(
                                RichText::new(format!("{cat}  ({})", in_bucket.len()))
                                    .small()
                                    .color(theme::TEXT_MUTED)
                                    .strong(),
                            );
                            for r in in_bucket {
                                let sel = selected_id.as_deref() == Some(r.id.as_str());
                                if master_row(ui, sel, r) {
                                    app.request_select_entity(Some(r.id.clone()));
                                }
                            }
                            ui.add_space(4.0);
                        }
                    } else {
                        for r in &rows {
                            let sel = selected_id.as_deref() == Some(r.id.as_str());
                            if master_row(ui, sel, r) {
                                app.request_select_entity(Some(r.id.clone()));
                            }
                        }
                    }
                });
        },
        detail_entity,
    );
}

// ─────────────────────────── Cast sub-tab ──────────────────────────

fn cast_tab(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.current_chapter.is_none() {
        empty_state(ui, "No chapter open.", "Open a chapter to see its cast.");
        return;
    }
    let frequencies = extract::frequency_map(&app.entity_hits);
    let in_scope = extract::by_kind(&frequencies, EntityKind::Character);
    if in_scope.is_empty() {
        empty_state(
            ui,
            "No known characters detected in this chapter.",
            "Either nobody is named yet, or the matcher hasn't been built for these characters.",
        );
        return;
    }

    let Some(book) = app.book.as_ref() else {
        return;
    };
    let mut rows: Vec<MasterRow> = in_scope
        .iter()
        .filter_map(|(id, count)| {
            book.entity(id).map(|e| MasterRow {
                id: e.id.clone(),
                name: e.name.clone(),
                aliases: e.aliases.clone(),
                category: e.category.clone(),
                count: Some(*count),
            })
        })
        .collect();

    let query = app.character_search.clone();
    if !query.is_empty() {
        rows.retain(|r| name_matches(&query, &r.name, &r.aliases));
    }

    let selected_id = app.selected_entity.clone();
    let detail_entity: Option<Entity> =
        selected_id.as_ref().and_then(|id| book.entity(id).cloned());

    master_detail_layout(
        app,
        ui,
        |ui, app| {
            search_input(ui, &mut app.character_search);
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("cast-list-scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if rows.is_empty() {
                        ui.label(RichText::new("no matches").small().color(theme::TEXT_MUTED));
                        return;
                    }
                    for r in &rows {
                        let sel = selected_id.as_deref() == Some(r.id.as_str());
                        if master_row(ui, sel, r) {
                            app.request_select_entity(Some(r.id.clone()));
                        }
                    }
                });
        },
        detail_entity,
    );
}

/// Two-column layout shared by Cast & Personae. Caller provides the master
/// column body; `detail_entity` (pre-cloned) drives the right column.
fn master_detail_layout(
    app: &mut CkWriterApp,
    ui: &mut egui::Ui,
    master: impl FnOnce(&mut egui::Ui, &mut CkWriterApp),
    detail_entity: Option<Entity>,
) {
    let list_w = master_list_width(ui);
    let total_h = ui.available_height();
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(list_w, total_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                master(ui, app);
            },
        );
        ui.separator();
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), total_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                detail_column(app, ui, detail_entity.as_ref());
            },
        );
    });
}

// ─────────────────────────── Proposals sub-tab ─────────────────────

fn proposals_tab(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.char_proposals.is_empty() {
        empty_state(
            ui,
            "No proposals yet.",
            "Click Find with AI above to extract characters from the open chapter.",
        );
        return;
    }

    let pending: Vec<usize> = app
        .char_proposals
        .iter()
        .enumerate()
        .filter(|(_, p)| p.status == ProposalStatus::Pending)
        .map(|(i, _)| i)
        .collect();
    let new_pending: usize = pending
        .iter()
        .filter(|&&i| matches!(app.char_proposals[i].verdict, ProposalVerdict::New))
        .count();
    let dup_pending: usize = pending.len() - new_pending;
    let added: usize = app
        .char_proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Added)
        .count();

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!(
                "{new_pending} new · {dup_pending} dup · {added} added"
            ))
            .small()
            .color(theme::TEXT_MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("clear").clicked() {
                app.clear_char_proposals();
            }
            ui.add_enabled_ui(new_pending > 0, |ui| {
                if ui
                    .button(RichText::new(format!("+ Add all {new_pending} new")).strong())
                    .clicked()
                {
                    app.accept_all_new_char_proposals();
                }
            });
        });
    });
    ui.add_space(4.0);

    let mut accept_idx: Option<usize> = None;
    let mut dismiss_idx: Option<usize> = None;
    for (idx, p) in app.char_proposals.iter().enumerate() {
        if p.status == ProposalStatus::Dismissed {
            continue;
        }
        proposal_card(ui, idx, p, &mut accept_idx, &mut dismiss_idx);
    }
    if let Some(i) = accept_idx {
        app.accept_char_proposal(i);
    }
    if let Some(i) = dismiss_idx {
        app.dismiss_char_proposal(i);
    }
}

fn proposal_card(
    ui: &mut egui::Ui,
    idx: usize,
    p: &crate::llm::characters::ProposedCharacter,
    accept_idx: &mut Option<usize>,
    dismiss_idx: &mut Option<usize>,
) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&p.raw.name)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    verdict_chip(ui, &p.verdict, p.status);
                });
            });
            if !p.raw.aliases.is_empty() {
                ui.label(
                    RichText::new(format!("aka {}", p.raw.aliases.join(", ")))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            if !p.raw.role.is_empty() {
                ui.label(RichText::new(&p.raw.role).small());
            }
            if !p.raw.evidence.is_empty() {
                ui.label(
                    RichText::new(format!("\u{201C}{}\u{201D}", short(&p.raw.evidence, 100)))
                        .italics()
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            ui.horizontal(|ui| {
                let already_added = p.status == ProposalStatus::Added;
                let is_dup = matches!(p.verdict, ProposalVerdict::Duplicate { .. });
                let can_add = !already_added && !is_dup;
                if ui.add_enabled(can_add, egui::Button::new("Add")).clicked() {
                    *accept_idx = Some(idx);
                }
                if !already_added && ui.button("Dismiss").clicked() {
                    *dismiss_idx = Some(idx);
                }
            });
        });
}

fn verdict_chip(ui: &mut egui::Ui, verdict: &ProposalVerdict, status: ProposalStatus) {
    let (label, fg, bg) = match (verdict, status) {
        (_, ProposalStatus::Added) => {
            ("added".to_string(), Color32::BLACK, theme::ENTITY_CHARACTER)
        }
        (ProposalVerdict::New, _) => ("new".to_string(), Color32::BLACK, theme::ACCENT),
        (ProposalVerdict::Duplicate { entity_name }, _) => (
            format!("dup · {entity_name}"),
            theme::TEXT_PRIMARY,
            theme::BG_INSET,
        ),
    };
    ui.label(RichText::new(label).small().color(fg).background_color(bg));
}

// ─────────────────────────── AI Output sub-tab ─────────────────────

fn ai_output_tab(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if let Some(s) = &app.char_stream {
        ui.label(
            RichText::new("● live extraction")
                .small()
                .color(theme::REVISION_VOICE),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new(&s.buffer)
                .monospace()
                .small()
                .color(theme::TEXT_MUTED),
        );
        return;
    }
    let Some(buf) = app.last_char_buffer.clone() else {
        empty_state(
            ui,
            "No AI run yet.",
            "Click Find with AI to send the chapter prose to the model.",
        );
        return;
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} bytes", buf.len()))
                .small()
                .color(theme::TEXT_MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("copy").clicked() {
                ui.ctx().copy_text(buf.clone());
            }
        });
    });
    ui.add_space(2.0);
    ui.label(
        RichText::new(&buf)
            .monospace()
            .small()
            .color(theme::TEXT_MUTED),
    );
}

// ─────────────────────────── Locations / AI / Notes ────────────────

fn show_locations(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.book.is_none() {
        return;
    }
    let frequencies = extract::frequency_map(&app.entity_hits);
    let in_scope = extract::by_kind(&frequencies, EntityKind::Location);
    egui::ScrollArea::vertical()
        .id_salt("locations-scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            if !in_scope.is_empty() {
                ui.label(
                    RichText::new("In this chapter")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                for (id, count) in in_scope {
                    location_row(app, ui, id, Some(count));
                }
                ui.add_space(8.0);
                ui.separator();
            }
            ui.label(RichText::new("All").small().color(theme::TEXT_MUTED));
            let ids: Vec<String> = {
                let book = app.book.as_ref().unwrap();
                book.entities_of(EntityKind::Location)
                    .iter()
                    .map(|e| e.id.clone())
                    .collect()
            };
            for id in ids {
                location_row(app, ui, &id, None);
            }
            ui.add_space(12.0);
            if ui.button("+ Add location").clicked() {
                app.create_blank_entity(EntityKind::Location);
            }
        });
}

fn location_row(app: &mut CkWriterApp, ui: &mut egui::Ui, id: &str, count: Option<usize>) {
    let Some(book) = &app.book else { return };
    let Some(e) = book.entity(id) else { return };
    let label = match count {
        Some(c) => format!("{}  ×{c}", e.name),
        None => e.name.clone(),
    };
    let is_selected = app.selected_entity.as_deref() == Some(id);
    if ui.selectable_label(is_selected, label).clicked() {
        app.request_select_entity(Some(id.to_string()));
    }
}

fn show_ai(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    use crate::llm::prompts::Pipeline;

    // Bottom strip: temperature slider for the coaching pipelines. Anchored
    // to the bottom so the revision-cards scroll area above keeps consuming
    // all remaining vertical space.
    egui::TopBottomPanel::bottom("ai-temperature-strip")
        .resizable(false)
        .show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut filter = app.settings.coach_filter_dismissed;
                if ui
                    .checkbox(&mut filter, "filter dismissed")
                    .on_hover_text(
                        "Hide dismissed cards from the panel. Recording is \
                         unconditional — switch this off in sealing mode to \
                         reconsider every dismissal (click a dismissed card \
                         to restore it).",
                    )
                    .changed()
                {
                    app.settings.coach_filter_dismissed = filter;
                    let _ = app.settings.save();
                    // Toggle is a panel-visibility filter, not an ingest
                    // filter — no pipeline needs to run. Rebuild from store.
                    app.rebuild_revisions_from_store();
                }
            });
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("temperature")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                let mut temp = app.settings.coach_temperature;
                let resp = ui.add(
                    egui::Slider::new(&mut temp, 0.0..=1.0)
                        .step_by(0.1)
                        .fixed_decimals(1),
                );
                if resp.changed() {
                    // Snap to the nearest 0.1 so toml round-trips don't drift
                    // the value by float noise (e.g. 0.30000000000000004).
                    app.settings.coach_temperature = (temp * 10.0).round() / 10.0;
                    let _ = app.settings.save();
                }
            });
            ui.add_space(4.0);
        });

    let busy = app.stream.is_some() || app.coach_run.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(
            !busy && app.current_chapter.is_some() && app.ollama_ok,
            |ui| {
                if ui.button("voice").clicked() {
                    app.run_pipeline(Pipeline::Voice);
                }
                if ui.button("show, don't tell").clicked() {
                    app.run_pipeline(Pipeline::ShowDontTell);
                }
                if ui.button("prose").clicked() {
                    app.run_pipeline(Pipeline::Prose);
                }
                if ui.button("spelling").clicked() {
                    app.run_pipeline(Pipeline::Spelling);
                }
            },
        );
    });
    if busy {
        let status = match &app.coach_run {
            Some(run) => format!(
                "● {} paragraph {}/{}",
                run.pipeline.label(),
                (run.current + 1).min(run.queue.len()),
                run.queue.len()
            ),
            None => "● running".to_string(),
        };
        ui.label(RichText::new(status).color(theme::REVISION_VOICE));
    } else if let Some(err) = &app.last_error {
        ui.label(RichText::new(err).color(Color32::LIGHT_RED));
    } else if app.ollama_ok {
        ui.label(RichText::new("ready").color(theme::TEXT_MUTED));
    } else {
        ui.label(RichText::new("ollama unreachable").color(Color32::LIGHT_RED));
    }
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("ai-stream-scroll")
        .max_height(160.0)
        .auto_shrink([false; 2])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if let Some(s) = &app.stream {
                ui.label(
                    RichText::new(&s.buffer)
                        .color(theme::TEXT_MUTED)
                        .monospace(),
                );
            } else if let Some(last) = &app.last_stream_buffer {
                ui.collapsing("last response", |ui| {
                    ui.label(RichText::new(last).color(theme::TEXT_MUTED).monospace());
                });
            } else {
                ui.label(
                    RichText::new("Click a pipeline to coach the current chapter.")
                        .color(theme::TEXT_MUTED),
                );
            }
        });
    ui.separator();

    show_paragraph_note_section(app, ui);
    ui.separator();

    let pending = app.revisions.len();
    ui.label(format!("{pending} pending suggestions"));
    ui.separator();

    let selected_id = app.selected_revision;
    let mut accept_id: Option<u32> = None;
    let mut dismiss_id: Option<u32> = None;
    let mut undismiss_id: Option<u32> = None;
    let mut select_id: Option<u32> = None;
    let mut save_dismissal_note: Option<(String, String)> = None;
    // Snapshot revisions so the iteration body can take `&mut app` borrows
    // (for the dismissal-note form HashMap) without colliding with the
    // outer `&app.revisions`. Cheap — Revision is small and bounded by
    // pending-suggestion count.
    let revisions_snapshot: Vec<crate::llm::revision::Revision> = app.revisions.clone();
    let live_dismissal_notes: HashMap<String, String> =
        collect_live_dismissal_notes(app, &revisions_snapshot);
    egui::ScrollArea::vertical()
        .id_salt("revisions-scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            // Bump the body/button font for the result-set so cards are
            // easier to scan than the rest of the panel.
            let style = ui.style_mut();
            style
                .text_styles
                .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
            style
                .text_styles
                .insert(egui::TextStyle::Button, egui::FontId::proportional(14.5));
            style
                .text_styles
                .insert(egui::TextStyle::Small, egui::FontId::proportional(12.5));

            for rev in &revisions_snapshot {
                let color = crate::ui::editor::revision_color(rev);
                let selected = selected_id == Some(rev.id);
                let card_resp = revision_card(ui, rev, color, selected);
                if card_resp.body_clicked {
                    // For dismissed cards in sealing mode, the body click is
                    // the un-dismiss affordance (matches the "I was wrong to
                    // dismiss this" mental model). Proposed cards get the
                    // usual select-and-jump behaviour.
                    if rev.is_dismissed {
                        undismiss_id = Some(rev.id);
                    } else {
                        select_id = Some(rev.id);
                    }
                }
                if card_resp.apply_clicked {
                    accept_id = Some(rev.id);
                }
                if card_resp.dismiss_clicked {
                    dismiss_id = Some(rev.id);
                }
                if rev.is_dismissed {
                    let live = live_dismissal_notes
                        .get(&rev.suggestion_id)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(saved) = dismissal_note_editor(
                        ui,
                        &mut app.dismissal_note_forms,
                        &rev.suggestion_id,
                        &live,
                    ) {
                        save_dismissal_note = Some((rev.suggestion_id.clone(), saved));
                    }
                }
            }
        });
    if let Some(id) = select_id {
        app.select_revision(id);
    }
    if let Some(id) = accept_id {
        app.accept_revision(id);
    }
    if let Some(id) = dismiss_id {
        app.dismiss_revision(id);
    }
    if let Some(id) = undismiss_id {
        app.undismiss_revision(id);
    }
    if let Some((suggestion_id, note)) = save_dismissal_note {
        app.save_dismissal_note(&suggestion_id, &note);
        if let Some(form) = app.dismissal_note_forms.get_mut(&suggestion_id) {
            form.mark_saved();
        }
    }
}

/// Read the live `dismissal_note` for every Dismissed revision in the
/// current snapshot, keyed by `suggestion_id`. Pulled in one immutable
/// pass so the iteration loop downstream can hold a `&mut` on the form
/// HashMap without re-borrowing the chapter store inside the loop.
fn collect_live_dismissal_notes(
    app: &CkWriterApp,
    revisions: &[crate::llm::revision::Revision],
) -> HashMap<String, String> {
    let Some(book) = app.book.as_ref() else {
        return HashMap::new();
    };
    let Some(ch) = app.current_chapter.as_ref() else {
        return HashMap::new();
    };
    let Some(store) = book.suggestions.for_chapter(&ch.folder, &ch.name) else {
        return HashMap::new();
    };
    revisions
        .iter()
        .filter(|r| r.is_dismissed)
        .filter_map(|r| {
            let rec = store.records.get(&r.suggestion_id)?;
            Some((
                r.suggestion_id.clone(),
                rec.dismissal_note.clone().unwrap_or_default(),
            ))
        })
        .collect()
}

/// Render the dismissal-reason textbox + Save / Revert beneath a Dismissed
/// card. Returns `Some(draft)` on Save click — caller persists. Lazy-seeds
/// the form on first render against the live note from disk; rebases when
/// clean so external rewrites flow in without prompting.
fn dismissal_note_editor(
    ui: &mut egui::Ui,
    forms: &mut HashMap<String, crate::ui::forms::Form<String>>,
    suggestion_id: &str,
    live_note: &str,
) -> Option<String> {
    let form = forms
        .entry(suggestion_id.to_string())
        .or_insert_with(|| crate::ui::forms::Form::new(&live_note.to_string()));
    form.rebase_if_clean(&live_note.to_string());
    let dirty = form.dirty();
    let mut save = false;
    let mut revert = false;
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 18,
            right: 0,
            top: 2,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("why dismissed?")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                if dirty {
                    ui.label(
                        RichText::new("●")
                            .color(theme::ACCENT)
                            .small()
                            .strong(),
                    );
                }
            });
            ui.add(
                egui::TextEdit::multiline(form.draft_mut())
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .hint_text(
                        "tell the AI why this is wrong feedback — \
                         it'll see the reason next run.",
                    ),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(dirty, egui::Button::new("Save"))
                    .clicked()
                {
                    save = true;
                }
                if ui
                    .add_enabled(dirty, egui::Button::new("Revert"))
                    .clicked()
                {
                    revert = true;
                }
            });
        });
    if revert {
        form.revert();
    }
    if save { Some(form.draft().clone()) } else { None }
}

/// Per-paragraph guidance note section above the coach cards (#0027).
/// The textbox is anchored to a single `paragraph_id` at a time — when the
/// editor cursor moves to a different paragraph we re-anchor only if the
/// form is currently clean, so a half-typed note isn't silently lost when
/// the writer clicks back into the editor mid-thought.
fn show_paragraph_note_section(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.book.is_none() || app.current_chapter.is_none() {
        return;
    }
    let cursor_pid = app.cursor_paragraph_id(ui.ctx());

    // Anchor resolution: if no form yet, anchor to the cursor's paragraph
    // (if any). If a form exists and is clean, rebase to the cursor's
    // paragraph so the textbox always tracks where the writer is. If
    // dirty, keep the previous anchor — the writer is mid-edit.
    let mut anchor_pid: Option<String> = match (&app.paragraph_note_form, cursor_pid.as_ref()) {
        (Some((stored, form)), Some(cursor)) if stored == cursor => Some(stored.clone()),
        (Some((stored, form)), Some(_)) if form.dirty() => Some(stored.clone()),
        (Some(_), Some(cursor)) => Some(cursor.clone()),
        (Some((stored, form)), None) if form.dirty() => Some(stored.clone()),
        (None, Some(cursor)) => Some(cursor.clone()),
        _ => None,
    };

    let Some(pid) = anchor_pid.take() else {
        ui.label(
            RichText::new("Click into a paragraph to add author guidance.")
                .small()
                .color(theme::TEXT_MUTED),
        );
        return;
    };

    let live_note = app
        .current_chapter
        .as_ref()
        .and_then(|c| c.meta.paragraph_notes.get(&pid).cloned())
        .unwrap_or_default();

    // Lazy-seed and rebase the form. `take` detaches it so the body
    // closure has clean borrow access; we put it back below.
    let mut entry = match app.paragraph_note_form.take() {
        Some((stored_pid, mut form)) if stored_pid == pid => {
            form.rebase_if_clean(&live_note);
            (stored_pid, form)
        }
        _ => (pid.clone(), crate::ui::forms::Form::new(&live_note)),
    };

    let cursor_elsewhere = cursor_pid.as_deref() != Some(entry.0.as_str());
    let dirty = entry.1.dirty();
    let mut save = false;
    let mut revert = false;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(10, 8))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Paragraph notes")
                        .small()
                        .strong()
                        .color(theme::TEXT_MUTED),
                );
                if dirty {
                    ui.label(
                        RichText::new("●")
                            .color(theme::ACCENT)
                            .small()
                            .strong(),
                    )
                    .on_hover_text("unsaved changes");
                }
                if cursor_elsewhere {
                    ui.label(
                        RichText::new("(cursor moved — finish or revert)")
                            .small()
                            .color(theme::ACCENT),
                    );
                }
            });
            ui.add(
                egui::TextEdit::multiline(entry.1.draft_mut())
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .hint_text(
                        "what is this paragraph supposed to do? \
                         the AI reads this before flagging.",
                    ),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(dirty, egui::Button::new("Save"))
                    .clicked()
                {
                    save = true;
                }
                if ui
                    .add_enabled(dirty, egui::Button::new("Revert"))
                    .clicked()
                {
                    revert = true;
                }
            });
        });

    let pid_for_save = entry.0.clone();
    let draft_for_save = entry.1.draft().clone();
    if save {
        app.save_paragraph_note(&pid_for_save, &draft_for_save);
        entry.1.mark_saved();
    }
    if revert {
        entry.1.revert();
    }
    app.paragraph_note_form = Some(entry);
}

struct RevisionCardEvents {
    body_clicked: bool,
    apply_clicked: bool,
    dismiss_clicked: bool,
}

fn revision_card(
    ui: &mut egui::Ui,
    rev: &crate::llm::revision::Revision,
    color: Color32,
    selected: bool,
) -> RevisionCardEvents {
    // Dismissed cards visible in sealing mode get a dimmed background and a
    // dedicated pill so the writer can tell them apart from live proposals.
    let bg = if selected {
        theme::REVISION_SELECTED_BG
    } else if rev.is_dismissed {
        theme::BG_INSET
    } else {
        Color32::TRANSPARENT
    };
    let dim = rev.is_dismissed;
    let body_text_color = if dim {
        theme::TEXT_MUTED
    } else {
        Color32::WHITE
    };
    let chip_color = if dim { theme::TEXT_MUTED } else { color };
    let mut body_clicked = false;
    let mut apply_clicked = false;
    let mut dismiss_clicked = false;
    egui::Frame::group(ui.style())
        .fill(bg)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            // Cards span the full panel width so they don't shrink-wrap to
            // their longest line and leave a ragged right edge.
            ui.set_min_width(ui.available_width());

            // Body region: only THIS area is click-sensitive for selecting
            // the card. Putting the buttons inside the same click rect would
            // let the body absorb their clicks (egui registers the parent's
            // sense after the children, so the parent wins on overlap).
            //
            // We use a Frame::none rather than ui.scope so the response carries
            // a real allocated rect that egui's hit-tester recognises — the
            // scope variant produced a hover-only allocation that
            // .interact(Sense::click()) didn't always upgrade reliably.
            let body_inner = egui::Frame::new()
                .inner_margin(egui::Margin::ZERO)
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let chip_label = if rev.kind == crate::llm::revision::FlagKind::Other {
                            rev.pipeline.label().to_string()
                        } else {
                            rev.kind.label().to_string()
                        };
                        ui.label(RichText::new(chip_label).color(chip_color).strong());
                        if rev.is_dismissed {
                            ui.label(
                                RichText::new("dismissed")
                                    .small()
                                    .color(Color32::BLACK)
                                    .background_color(theme::TEXT_MUTED),
                            );
                        }
                        if rev.anchor.is_none() {
                            ui.label(
                                RichText::new("(unanchored)")
                                    .small()
                                    .color(Color32::LIGHT_RED),
                            );
                        }
                    });
                    ui.label(
                        RichText::new(format!("\u{201C}{}\u{201D}", short(&rev.quote, 100)))
                            .italics()
                            .color(theme::TEXT_MUTED),
                    );
                    if !rev.why.is_empty() {
                        ui.label(RichText::new(&rev.why).color(if dim {
                            theme::TEXT_MUTED
                        } else {
                            theme::TEXT_PRIMARY
                        }));
                    }
                    if !rev.suggestion.is_empty() {
                        ui.label(RichText::new(&rev.suggestion).color(body_text_color));
                    }
                });
            let body_resp = body_inner.response.interact(egui::Sense::click());
            body_clicked = body_resp.clicked();
            if body_clicked {
                log::info!(
                    "revision card body clicked: id={} pipeline={} kind={:?} anchor={:?} dismissed={}",
                    rev.id,
                    rev.pipeline.label(),
                    rev.kind,
                    rev.anchor,
                    rev.is_dismissed,
                );
            }

            // Action row lives OUTSIDE the body's click rect, so button
            // clicks don't double as a "select card" click. Dismissed cards
            // hide the destructive action (it's already dismissed); body
            // click handles the un-dismiss.
            if !rev.is_dismissed {
                ui.horizontal(|ui| {
                    let has_anchor = rev.anchor.is_some();
                    let can_apply = !rev.suggestion.is_empty() && has_anchor;
                    if ui
                        .add_enabled(can_apply, egui::Button::new("Apply"))
                        .clicked()
                    {
                        apply_clicked = true;
                    }
                    if ui.button("Dismiss").clicked() {
                        dismiss_clicked = true;
                    }
                });
            } else {
                ui.label(
                    RichText::new("click to restore")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
        });
    RevisionCardEvents {
        body_clicked,
        apply_clicked,
        dismiss_clicked,
    }
}

fn show_chat(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.book.is_none() {
        empty_state(ui, "Open a book", "Load a book to chat about a chapter.");
        return;
    }
    if app.current_chapter.is_none() {
        empty_state(
            ui,
            "Open a chapter",
            "Pick a chapter and the model will read it as context.",
        );
        return;
    }

    let busy = app.chat_stream.is_some();
    let chapter_label = app
        .current_chapter
        .as_ref()
        .map(|c| c.display_title.clone())
        .unwrap_or_default();

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("about: {chapter_label}"))
                .small()
                .color(theme::TEXT_MUTED),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(!busy && !app.chat_messages.is_empty(), |ui| {
                if ui.small_button("clear").clicked() {
                    app.reset_chat();
                }
            });
        });
    });

    if busy {
        ui.label(
            RichText::new("● thinking…")
                .small()
                .color(theme::REVISION_VOICE),
        );
    } else if !app.ollama_ok {
        ui.label(
            RichText::new("ollama unreachable")
                .small()
                .color(Color32::LIGHT_RED),
        );
    } else if let Some(err) = &app.chat_error {
        ui.label(RichText::new(err).small().color(Color32::LIGHT_RED));
    }
    ui.separator();

    // Reserve a fixed slice for the input row at the bottom; the transcript
    // takes the rest. Without this the ScrollArea expands and the input
    // disappears below the fold.
    let input_h: f32 = 96.0;
    let total_h = ui.available_height();
    let transcript_h = (total_h - input_h - 8.0).max(120.0);

    egui::ScrollArea::vertical()
        .id_salt("chat-transcript-scroll")
        .max_height(transcript_h)
        .auto_shrink([false; 2])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if app.chat_messages.is_empty() && app.chat_pending_assistant.is_empty() {
                empty_state(
                    ui,
                    "Ask about this chapter.",
                    "Try: \"Where does the pacing sag?\" or \"Is Kira's voice consistent here?\"",
                );
                return;
            }
            // Bubble body inherits the editor's reading font + size (#0020):
            // chat is a reading surface and the user is dyslexic, so a
            // 13 px default proportional bubble is too small for sustained
            // assistant replies.
            let reading_font = theme::reading_family(app.settings.reading_font);
            let reading_size = app.settings.reading_font_size;
            for msg in &app.chat_messages {
                chat_bubble(ui, &msg.role, &msg.content, &reading_font, reading_size);
            }
            if !app.chat_pending_assistant.is_empty() {
                chat_bubble(
                    ui,
                    "assistant",
                    &app.chat_pending_assistant,
                    &reading_font,
                    reading_size,
                );
            }
        });

    ui.separator();

    let mut send_now = false;
    ui.horizontal(|ui| {
        let resp = ui.add_sized(
            egui::vec2(ui.available_width() - 80.0, 64.0),
            egui::TextEdit::multiline(&mut app.chat_input)
                .desired_rows(3)
                .hint_text("ask the model about this chapter…"),
        );
        // Cmd/Ctrl+Enter sends; plain Enter inserts a newline so multi-line
        // questions are easy to write.
        if resp.has_focus() && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
        {
            send_now = true;
        }
        ui.vertical(|ui| {
            ui.add_enabled_ui(!busy && !app.chat_input.trim().is_empty(), |ui| {
                if ui
                    .add_sized(egui::vec2(72.0, 28.0), egui::Button::new("Send"))
                    .clicked()
                {
                    send_now = true;
                }
            });
            ui.label(RichText::new("⌘↵").small().color(theme::TEXT_MUTED));
        });
    });
    if send_now {
        app.send_chat_message();
    }
}

fn chat_bubble(
    ui: &mut egui::Ui,
    role: &str,
    content: &str,
    body_family: &egui::FontFamily,
    body_size: f32,
) {
    let (label, fg) = match role {
        "user" => ("you", theme::ACCENT),
        "assistant" => ("ai", theme::REVISION_VOICE),
        other => (other, theme::TEXT_MUTED),
    };
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(label).small().color(fg).strong());
            ui.label(
                RichText::new(content)
                    .color(theme::TEXT_PRIMARY)
                    .font(egui::FontId::new(body_size, body_family.clone())),
            );
        });
    ui.add_space(4.0);
}

fn show_chapter(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    let Some(live) = app.current_chapter.as_ref().map(|c| c.meta.clone()) else {
        ui.label(RichText::new("Open a chapter first.").color(theme::TEXT_MUTED));
        return;
    };

    // Detach the form from app so the body closure has free access to the
    // rest of `app`. Lazily seed on first render; rebase the snapshot from
    // the live ChapterMeta while clean so external updates (e.g. word_count
    // recompute on save) flow in without prompting the user.
    let mut form = app
        .chapter_form
        .take()
        .unwrap_or_else(|| crate::ui::forms::Form::new(&live));
    form.rebase_if_clean(&live);

    let action = egui::ScrollArea::vertical()
        .id_salt("chapter-tab-scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            crate::ui::forms::render(&mut form, "Chapter info", ui, |ui, draft| {
                ui.label(
                    RichText::new("summary")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut draft.summary)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("one or two sentences — what happens in this chapter"),
                );
                ui.add_space(6.0);

                ui.label(RichText::new("goals").small().color(theme::TEXT_MUTED));
                ui.add(
                    egui::TextEdit::multiline(&mut draft.goals)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("what this chapter needs to accomplish"),
                );
                ui.add_space(6.0);

                ui.label(
                    RichText::new("plot notes / scratchpad")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut draft.plot_notes)
                        .desired_rows(10)
                        .desired_width(f32::INFINITY)
                        .hint_text("free-form notes — discovery writing, beats, reminders"),
                );

                ui.add_space(10.0);
                ui.separator();
                // Render stats off the form's own draft so the editable
                // fields and read-only stats are guaranteed to share one
                // generation of state — no drift.
                chapter_readonly_stats(ui, draft);
            })
        })
        .inner;

    app.chapter_form = Some(form);
    match action {
        crate::ui::forms::FormAction::Save => app.save_chapter_form(),
        crate::ui::forms::FormAction::Revert => {
            if let Some(f) = app.chapter_form.as_mut() {
                f.revert();
            }
        }
        crate::ui::forms::FormAction::None => {}
    }
}

fn chapter_readonly_stats(ui: &mut egui::Ui, meta: &crate::book::chapter_meta::ChapterMeta) {
    ui.label(RichText::new("stats").small().color(theme::TEXT_MUTED));
    egui::Grid::new("chapter-tab-stats")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(RichText::new("word count").small().color(theme::TEXT_MUTED));
            ui.label(RichText::new(meta.word_count.to_string()));
            ui.end_row();

            ui.label(
                RichText::new("voice score")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let voice = meta
                .voice_score
                .map(|s| s.to_string())
                .unwrap_or_else(|| "—".to_string());
            ui.label(RichText::new(voice));
            ui.end_row();

            ui.label(
                RichText::new("last coached")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            ui.label(RichText::new(format_unix_seconds(meta.last_coached_at)));
            ui.end_row();

            ui.label(
                RichText::new("locked paragraphs")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let locked = meta.paragraphs.iter().filter(|p| p.locked).count();
            let total = meta.paragraphs.len();
            ui.label(RichText::new(format!("{locked} / {total}")));
            ui.end_row();
        });
}

fn format_unix_seconds(ts: Option<i64>) -> String {
    let Some(t) = ts else {
        return "—".to_string();
    };
    // Avoid pulling in chrono just for one timestamp. ISO-ish UTC is plenty
    // here — the writer only needs to know whether the score is fresh or
    // stale, not the exact minute.
    let days = t / 86_400;
    // 1970-01-01 was a Thursday; that's not what we render, just the date.
    // Use a simple proleptic Gregorian calc.
    let (y, m, d) = days_to_ymd(days);
    let secs_of_day = t.rem_euclid(86_400);
    let h = secs_of_day / 3600;
    let mi = (secs_of_day % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02} UTC")
}

/// Convert a count of days since 1970-01-01 to a (year, month, day) tuple in
/// the proleptic Gregorian calendar. Lifted from Howard Hinnant's date
/// algorithm; kept here so the Chapter tab doesn't depend on chrono.
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

// ─────────────────────────── shared helpers ────────────────────────

fn short(s: &str, n: usize) -> String {
    let mut t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        t.push('\u{2026}');
    }
    t
}

fn empty_state(ui: &mut egui::Ui, headline: &str, hint: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(headline).color(theme::TEXT_MUTED).strong());
    ui.label(RichText::new(hint).small().color(theme::TEXT_MUTED));
}
