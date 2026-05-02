use crate::app::CkWriterApp;
use crate::book::entity::{Entity, EntityKind};
use crate::extract;
use crate::llm::characters::{ProposalStatus, ProposalVerdict};
use crate::theme;
use egui::{Color32, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Characters,
    Locations,
    AI,
    Chat,
    Notes,
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
            (Tab::Notes, "Notes"),
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
        Tab::Notes => show_notes(app, ui),
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
        ui.label(RichText::new("● extracting…").small().color(theme::REVISION_VOICE));
    } else if let Some(err) = &app.char_extract_error {
        ui.label(RichText::new(err).small().color(Color32::LIGHT_RED));
    } else if !app.ollama_ok {
        ui.label(RichText::new("ollama unreachable").small().color(Color32::LIGHT_RED));
    } else if app.current_chapter.is_none() {
        ui.label(RichText::new("open a chapter to extract").small().color(theme::TEXT_MUTED));
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
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        if let Some(c) = row.count {
                            ui.label(
                                RichText::new(format!("\u{00D7}{c}"))
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        }
                    },
                );
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
    let Some(book) = app.book.as_ref() else { return };
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
    let detail_entity: Option<Entity> = selected_id
        .as_ref()
        .and_then(|id| book.entity(id).cloned());

    master_detail_layout(app, ui, |ui, app| {
        // Master column
        search_input(ui, &mut app.character_search);
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("personae-list-scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(
                        RichText::new("no matches")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
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
                                app.selected_entity = Some(r.id.clone());
                            }
                        }
                        ui.add_space(4.0);
                    }
                } else {
                    for r in &rows {
                        let sel = selected_id.as_deref() == Some(r.id.as_str());
                        if master_row(ui, sel, r) {
                            app.selected_entity = Some(r.id.clone());
                        }
                    }
                }
            });
    }, detail_entity);
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

    let Some(book) = app.book.as_ref() else { return };
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
    let detail_entity: Option<Entity> = selected_id
        .as_ref()
        .and_then(|id| book.entity(id).cloned());

    master_detail_layout(app, ui, |ui, app| {
        search_input(ui, &mut app.character_search);
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("cast-list-scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if rows.is_empty() {
                    ui.label(
                        RichText::new("no matches")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                    return;
                }
                for r in &rows {
                    let sel = selected_id.as_deref() == Some(r.id.as_str());
                    if master_row(ui, sel, r) {
                        app.selected_entity = Some(r.id.clone());
                    }
                }
            });
    }, detail_entity);
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
                ui.label(RichText::new(&p.raw.name).strong().color(theme::TEXT_PRIMARY));
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
        (_, ProposalStatus::Added) => (
            "added".to_string(),
            Color32::BLACK,
            theme::ENTITY_CHARACTER,
        ),
        (ProposalVerdict::New, _) => ("new".to_string(), Color32::BLACK, theme::ACCENT),
        (ProposalVerdict::Duplicate { entity_name }, _) => (
            format!("dup · {entity_name}"),
            theme::TEXT_PRIMARY,
            theme::BG_INSET,
        ),
    };
    ui.label(
        RichText::new(label)
            .small()
            .color(fg)
            .background_color(bg),
    );
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
                ui.label(RichText::new("In this chapter").small().color(theme::TEXT_MUTED));
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
        app.selected_entity = Some(id.to_string());
    }
}

fn show_ai(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    use crate::llm::prompts::Pipeline;

    let busy = app.stream.is_some();
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled_ui(!busy && app.current_chapter.is_some() && app.ollama_ok, |ui| {
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
        });
    });
    if busy {
        ui.label(RichText::new("● running").color(theme::REVISION_VOICE));
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
                ui.label(RichText::new(&s.buffer).color(theme::TEXT_MUTED).monospace());
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

    let pending = app.revisions.len();
    ui.label(format!("{pending} pending suggestions"));
    ui.separator();

    let selected_id = app.selected_revision;
    let mut accept_id: Option<u32> = None;
    let mut dismiss_id: Option<u32> = None;
    let mut select_id: Option<u32> = None;
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

            for rev in &app.revisions {
                let color = crate::ui::editor::revision_color(rev);
                let selected = selected_id == Some(rev.id);
                let card_resp = revision_card(ui, rev, color, selected);
                if card_resp.body_clicked {
                    select_id = Some(rev.id);
                }
                if card_resp.apply_clicked {
                    accept_id = Some(rev.id);
                }
                if card_resp.dismiss_clicked {
                    dismiss_id = Some(rev.id);
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
    let bg = if selected {
        theme::REVISION_SELECTED_BG
    } else {
        Color32::TRANSPARENT
    };
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
            let body_resp = ui
                .scope(|ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let chip_label = if rev.kind == crate::llm::revision::FlagKind::Other {
                            rev.pipeline.label().to_string()
                        } else {
                            rev.kind.label().to_string()
                        };
                        ui.label(RichText::new(chip_label).color(color).strong());
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
                        ui.label(&rev.why);
                    }
                    if !rev.suggestion.is_empty() {
                        ui.label(RichText::new(&rev.suggestion).color(Color32::WHITE));
                    }
                })
                .response
                .interact(egui::Sense::click());
            body_clicked = body_resp.clicked();

            // Action row lives OUTSIDE the body's click rect, so button
            // clicks don't double as a "select card" click.
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
            for msg in &app.chat_messages {
                chat_bubble(ui, &msg.role, &msg.content);
            }
            if !app.chat_pending_assistant.is_empty() {
                chat_bubble(ui, "assistant", &app.chat_pending_assistant);
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
        if resp.has_focus()
            && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter))
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
            ui.label(
                RichText::new("⌘↵")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        });
    });
    if send_now {
        app.send_chat_message();
    }
}

fn chat_bubble(ui: &mut egui::Ui, role: &str, content: &str) {
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
            ui.label(RichText::new(content).color(theme::TEXT_PRIMARY));
        });
    ui.add_space(4.0);
}

fn show_notes(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    ui.label(RichText::new("Per-chapter notes").small().color(theme::TEXT_MUTED));
    if app.current_chapter.is_none() {
        ui.label(RichText::new("Open a chapter first.").color(theme::TEXT_MUTED));
        return;
    }
    let mut changed = false;
    let resp = ui.add_sized(
        ui.available_size(),
        egui::TextEdit::multiline(&mut app.notes_text)
            .desired_rows(20)
            .hint_text("scratchpad — saved next to the chapter as .notes.md"),
    );
    if resp.changed() {
        changed = true;
    }
    if changed {
        app.notes_dirty = true;
    }
    if ui.button("Save notes").clicked() {
        app.save_notes();
    }
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
