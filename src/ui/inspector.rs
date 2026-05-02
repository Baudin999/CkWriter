use crate::app::CkWriterApp;
use crate::book::entity::{Entity, EntityKind, ProgressionEntry, Relation};
use crate::theme;
use egui::RichText;

/// Bottom-of-rail entry point used when no inline detail is active. Reads the
/// current selection and delegates to `render_detail`.
pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    let Some(id) = app.selected_entity.clone() else {
        return;
    };
    let Some(book) = &app.book else { return };
    let Some(existing) = book.entity(&id).cloned() else {
        return;
    };
    render_detail(app, ui, &existing);
}

/// Render the full detail / edit view for an entity. Bumps body + small text
/// sizes inside its own UI scope so the form is readable in either the
/// bottom-rail layout or the master/detail column inside Cast/Personae.
pub fn render_detail(app: &mut CkWriterApp, ui: &mut egui::Ui, existing: &Entity) {
    ui.scope(|ui| {
        // Defaults are ~14 / ~10. Bump for readability inside the detail view.
        let style = ui.style_mut();
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(13.5));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));

        let id = existing.id.clone();
        ui.heading(&existing.name);
        ui.separator();

        // Take the current dirty buffer if it's for this entity, otherwise start
        // from the saved version. This keeps edits coherent across re-renders.
        let mut e = match app.entity_dirty.as_ref() {
            Some(d) if d.id == existing.id => d.clone(),
            _ => existing.clone(),
        };
        let mut changed = false;

        egui::Grid::new(format!("inspector-grid-{id}"))
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                field(ui, "name", &mut e.name, &mut changed);
                field(ui, "role", &mut e.role, &mut changed);
                field(ui, "age", &mut e.age, &mut changed);
                field(ui, "tone", &mut e.tone, &mut changed);
                category_row(ui, app, &mut e.category, &mut changed);
            });

        ui.label(RichText::new("voice notes").small().color(theme::TEXT_MUTED));
        if ui
            .add(
                egui::TextEdit::multiline(&mut e.voice_notes)
                    .desired_rows(2)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            changed = true;
        }

        ui.label(
            RichText::new("aliases (comma-separated)")
                .small()
                .color(theme::TEXT_MUTED),
        );
        let mut aliases_str = e.aliases.join(", ");
        if ui
            .add(egui::TextEdit::singleline(&mut aliases_str).desired_width(f32::INFINITY))
            .changed()
        {
            e.aliases = aliases_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            changed = true;
        }

        ui.label(
            RichText::new("free text / lore")
                .small()
                .color(theme::TEXT_MUTED),
        );
        if ui
            .add(
                egui::TextEdit::multiline(&mut e.free_text)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            changed = true;
        }

        ui.add_space(6.0);
        relations_section(ui, app, &existing.id, &mut e.relations, &mut changed);

        if changed {
            app.entity_dirty = Some(e);
        }

        ui.add_space(6.0);
        let can_save = app.entity_dirty.is_some();
        ui.horizontal(|ui| {
            if ui.add_enabled(can_save, egui::Button::new("Save")).clicked() {
                app.commit_entity_edit();
            }
            if ui.add_enabled(can_save, egui::Button::new("Revert")).clicked() {
                app.entity_dirty = None;
            }
            if ui.button("Close").clicked() {
                app.selected_entity = None;
                app.entity_dirty = None;
            }
        });

        if existing.kind == EntityKind::Character {
            progression_section(app, ui, &existing.id, &existing.progression);
        }
        show_appearances(app, ui, &id);
    });
}

fn progression_section(
    app: &mut CkWriterApp,
    ui: &mut egui::Ui,
    entity_id: &str,
    entries: &[ProgressionEntry],
) {
    ui.add_space(6.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("progression")
                .small()
                .color(theme::TEXT_MUTED)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let busy = app.progression_stream.is_some();
            let in_flight_for_this = busy
                && app
                    .progression_target
                    .as_ref()
                    .map(|(id, _)| id == entity_id)
                    .unwrap_or(false);
            let chapter_open = app.current_chapter.is_some();
            let can_run = !busy && chapter_open && app.ollama_ok;
            let label = if in_flight_for_this {
                "● tracking…"
            } else {
                "Track for current chapter"
            };
            if ui
                .add_enabled(can_run, egui::Button::new(label))
                .clicked()
            {
                app.track_progression_for(entity_id);
            }
        });
    });
    if let Some(s) = &app.progression_status {
        ui.label(RichText::new(s).small().color(theme::TEXT_MUTED));
    }

    if entries.is_empty() {
        ui.label(
            RichText::new("No snapshots yet. Open a chapter and click Track.")
                .small()
                .color(theme::TEXT_MUTED),
        );
        return;
    }
    for entry in entries {
        progression_card(ui, entry);
    }
}

fn progression_card(ui: &mut egui::Ui, e: &ProgressionEntry) {
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(egui::CornerRadius::same(4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&e.chapter)
                        .small()
                        .color(theme::ACCENT)
                        .strong(),
                );
                if !e.tone.is_empty() {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(&e.tone)
                                    .small()
                                    .italics()
                                    .color(theme::TEXT_MUTED),
                            );
                        },
                    );
                }
            });
            if !e.situation.is_empty() {
                ui.label(RichText::new(&e.situation).small());
            }
            if !e.voice_summary.is_empty() {
                ui.label(
                    RichText::new(format!("voice: {}", e.voice_summary))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
            if !e.notable_changes.is_empty() {
                ui.label(
                    RichText::new(format!("\u{0394} {}", e.notable_changes))
                        .small()
                        .color(theme::REVISION_VOICE),
                );
            }
        });
}

fn category_row(
    ui: &mut egui::Ui,
    app: &CkWriterApp,
    category: &mut String,
    changed: &mut bool,
) {
    ui.label("category");
    let categories: Vec<String> = app
        .book
        .as_ref()
        .map(|b| b.data.categories.clone())
        .unwrap_or_default();
    // If the entity holds a category not in the configured list (e.g. the
    // writer edited book.json and renamed it), keep showing it as a valid
    // selection so we don't silently drop the value.
    let mut shown: Vec<String> = categories;
    if !category.is_empty() && !shown.iter().any(|c| c == category) {
        shown.push(category.clone());
    }

    let label = if category.is_empty() {
        "(none)".to_string()
    } else {
        category.clone()
    };
    egui::ComboBox::from_id_salt("inspector-category")
        .selected_text(label)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(category.is_empty(), "(none)")
                .clicked()
                && !category.is_empty()
            {
                category.clear();
                *changed = true;
            }
            for c in &shown {
                if ui
                    .selectable_label(category == c, c)
                    .clicked()
                    && category != c
                {
                    *category = c.clone();
                    *changed = true;
                }
            }
        });
    ui.end_row();
}

fn relations_section(
    ui: &mut egui::Ui,
    app: &CkWriterApp,
    self_id: &str,
    relations: &mut Vec<Relation>,
    changed: &mut bool,
) {
    let Some(book) = app.book.as_ref() else { return };
    ui.separator();
    ui.label(
        RichText::new("relationships")
            .small()
            .color(theme::TEXT_MUTED)
            .strong(),
    );

    let kinds: Vec<String> = book.data.relation_kinds.clone();
    // Targets = every other entity (characters first, then locations, …).
    let targets: Vec<(String, String)> = book
        .entities
        .by_id
        .values()
        .filter(|t| t.id != self_id)
        .map(|t| (t.id.clone(), entity_label(t)))
        .collect();

    let mut to_remove: Option<usize> = None;
    for (idx, rel) in relations.iter_mut().enumerate() {
        ui.horizontal_wrapped(|ui| {
            relation_kind_combo(ui, idx, &kinds, &mut rel.kind, changed);
            relation_target_combo(ui, idx, &targets, &mut rel.id, changed);
            if ui.small_button("\u{2715}").on_hover_text("remove").clicked() {
                to_remove = Some(idx);
            }
        });
        if let Some(inv) = book.data.inverse_relation(&rel.kind) {
            if !rel.id.is_empty() {
                let target_name = targets
                    .iter()
                    .find(|(id, _)| id == &rel.id)
                    .map(|(_, n)| n.as_str())
                    .unwrap_or(rel.id.as_str());
                ui.label(
                    RichText::new(format!("\u{21B3} mirrors as \u{201C}{inv}\u{201D} on {target_name}"))
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            }
        }
    }
    if let Some(i) = to_remove {
        relations.remove(i);
        *changed = true;
    }

    if ui.button("+ Add relation").clicked() {
        relations.push(Relation {
            kind: kinds.first().cloned().unwrap_or_default(),
            id: String::new(),
        });
        *changed = true;
    }
}

fn relation_kind_combo(
    ui: &mut egui::Ui,
    idx: usize,
    kinds: &[String],
    kind: &mut String,
    changed: &mut bool,
) {
    let selected_text = if kind.is_empty() {
        "kind…".to_string()
    } else {
        kind.clone()
    };
    egui::ComboBox::from_id_salt(("rel-kind", idx))
        .selected_text(selected_text)
        .width(140.0)
        .show_ui(ui, |ui| {
            for k in kinds {
                if ui.selectable_label(kind == k, k).clicked() && kind != k {
                    *kind = k.clone();
                    *changed = true;
                }
            }
            ui.separator();
            ui.label(RichText::new("custom").small().color(theme::TEXT_MUTED));
            if ui
                .add(egui::TextEdit::singleline(kind).desired_width(120.0).hint_text("free text"))
                .changed()
            {
                *changed = true;
            }
        });
}

fn relation_target_combo(
    ui: &mut egui::Ui,
    idx: usize,
    targets: &[(String, String)],
    target_id: &mut String,
    changed: &mut bool,
) {
    let selected_label = if target_id.is_empty() {
        "target…".to_string()
    } else {
        targets
            .iter()
            .find(|(id, _)| id == target_id)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| target_id.clone())
    };
    egui::ComboBox::from_id_salt(("rel-target", idx))
        .selected_text(selected_label)
        .width(180.0)
        .show_ui(ui, |ui| {
            for (id, label) in targets {
                if ui.selectable_label(target_id == id, label).clicked() && target_id != id {
                    *target_id = id.clone();
                    *changed = true;
                }
            }
        });
}

fn entity_label(e: &Entity) -> String {
    match e.kind {
        EntityKind::Character => e.name.clone(),
        EntityKind::Location => format!("{} (loc)", e.name),
        EntityKind::Event => format!("{} (ev)", e.name),
        EntityKind::Timeline => format!("{} (tl)", e.name),
    }
}

fn show_appearances(app: &mut CkWriterApp, ui: &mut egui::Ui, id: &str) {
    let Some(idx) = app.char_index.as_ref() else { return };
    let occurrences = idx.for_entity(id);
    if occurrences.is_empty() {
        return;
    }
    ui.add_space(8.0);
    ui.separator();
    let chapters = idx.distinct_chapter_count(id);
    let total = idx.total_occurrences(id);
    ui.label(
        RichText::new(format!("Appears in {chapters} chapter(s), {total} hits"))
            .small()
            .color(theme::TEXT_MUTED),
    );

    let mut jump: Option<(std::path::PathBuf, u32)> = None;
    egui::ScrollArea::vertical()
        .id_salt(("appearances", id.to_string()))
        .max_height(220.0)
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for occ in occurrences {
                let header = format!("{}  \u{00B7}  L{}", occ.chapter_title, occ.line);
                if ui
                    .link(RichText::new(header).color(theme::ACCENT).small())
                    .clicked()
                {
                    jump = Some((occ.chapter_path.clone(), occ.line));
                }
                ui.label(
                    RichText::new(&occ.snippet)
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(2.0);
            }
        });
    if let Some((file, line)) = jump {
        app.jump_to_source(&file, line);
    }
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, changed: &mut bool) {
    ui.label(label);
    if ui
        .add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY))
        .changed()
    {
        *changed = true;
    }
    ui.end_row();
}
