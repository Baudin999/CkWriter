use crate::app::CkWriterApp;
use crate::settings::ReadingFont;
use crate::theme;
use egui::RichText;

pub fn show(app: &mut CkWriterApp, ctx: &egui::Context) {
    if !app.show_settings {
        return;
    }
    let mut open = true;
    let mut close = false;
    let mut changed = false;

    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(440.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("model").small().color(theme::TEXT_MUTED));
            if app.available_models.is_empty() {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.settings.model)
                        .desired_width(400.0)
                        .hint_text("e.g. gemma4:26b"),
                );
                if resp.changed() {
                    changed = true;
                }
                ui.label(
                    RichText::new("ollama not reachable — type a model name")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
            } else {
                let current = app.settings.model.clone();
                egui::ComboBox::from_id_salt("settings_model")
                    .selected_text(&current)
                    .width(400.0)
                    .show_ui(ui, |ui| {
                        for name in &app.available_models {
                            if ui.selectable_label(name == &current, name).clicked()
                                && app.settings.model != *name
                            {
                                app.settings.model = name.clone();
                                changed = true;
                            }
                        }
                    });
            }
            if let Some(book_model) = app.book.as_ref().and_then(|b| b.config.model.clone()) {
                ui.label(
                    RichText::new(format!(
                        "current book overrides model on open: {book_model}"
                    ))
                    .small()
                    .color(theme::TEXT_MUTED),
                );
            }

            ui.add_space(8.0);
            ui.label(RichText::new("ollama url").small().color(theme::TEXT_MUTED));
            let resp = ui
                .add(egui::TextEdit::singleline(&mut app.settings.ollama_url).desired_width(400.0));
            if resp.changed() {
                changed = true;
            }

            ui.add_space(12.0);
            ui.label(
                RichText::new("Reading (app-wide)")
                    .small()
                    .strong()
                    .color(theme::TEXT_MUTED),
            );

            ui.add_space(4.0);
            ui.label(RichText::new("font").small().color(theme::TEXT_MUTED));
            let current_font_label = match app.settings.reading_font {
                ReadingFont::AtkinsonHyperlegible => "Atkinson Hyperlegible",
                ReadingFont::OpenDyslexic => "OpenDyslexic",
                ReadingFont::IaWriterQuattro => "iA Writer Quattro",
            };
            egui::ComboBox::from_id_salt("settings_reading_font")
                .selected_text(current_font_label)
                .width(400.0)
                .show_ui(ui, |ui| {
                    for (variant, label) in [
                        (
                            ReadingFont::AtkinsonHyperlegible,
                            "Atkinson Hyperlegible",
                        ),
                        (ReadingFont::OpenDyslexic, "OpenDyslexic"),
                        (ReadingFont::IaWriterQuattro, "iA Writer Quattro"),
                    ] {
                        let selected = app.settings.reading_font == variant;
                        if ui.selectable_label(selected, label).clicked() && !selected {
                            app.settings.reading_font = variant;
                            changed = true;
                        }
                    }
                });

            ui.add_space(6.0);
            ui.label(
                RichText::new("font size — body")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let resp = ui.add(
                egui::Slider::new(&mut app.settings.font_size_normal, 12.0..=28.0)
                    .step_by(1.0)
                    .suffix(" px"),
            );
            if resp.changed() {
                changed = true;
            }

            ui.add_space(6.0);
            ui.label(
                RichText::new("font size — heading")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let resp = ui.add(
                egui::Slider::new(&mut app.settings.font_size_header, 14.0..=34.0)
                    .step_by(1.0)
                    .suffix(" px"),
            );
            if resp.changed() {
                changed = true;
            }

            ui.add_space(6.0);
            ui.label(
                RichText::new("font size — info")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let resp = ui.add(
                egui::Slider::new(&mut app.settings.font_size_info, 10.0..=18.0)
                    .step_by(1.0)
                    .suffix(" px"),
            );
            if resp.changed() {
                changed = true;
            }

            ui.add_space(12.0);
            ui.label(
                RichText::new("Editor")
                    .small()
                    .strong()
                    .color(theme::TEXT_MUTED),
            );

            ui.add_space(4.0);
            ui.label(
                RichText::new("column width")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let resp = ui.add(
                egui::Slider::new(&mut app.settings.editor_column_width, 480.0..=1000.0)
                    .step_by(10.0)
                    .suffix(" px"),
            );
            if resp.changed() {
                changed = true;
            }

            ui.add_space(6.0);
            ui.label(
                RichText::new("line height")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let resp = ui.add(
                egui::Slider::new(&mut app.settings.editor_line_height_mult, 1.2..=2.2)
                    .step_by(0.1)
                    .suffix("×"),
            );
            if resp.changed() {
                changed = true;
            }

            ui.add_space(6.0);
            ui.label(
                RichText::new("letter spacing")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            let resp = ui.add(
                egui::Slider::new(&mut app.settings.editor_letter_spacing, 0.0..=1.5)
                    .step_by(0.1)
                    .suffix(" px"),
            );
            if resp.changed() {
                changed = true;
            }

            // === Project folders (per-book; #0033) ===
            // Only renders when a book is open, since these settings live in
            // the book's `Info/index.json`, not in app-wide settings.toml.
            let mut project_changed = false;
            if let Some(book) = app.book.as_mut() {
                ui.add_space(12.0);
                ui.separator();
                ui.label(
                    RichText::new("Project folders")
                        .small()
                        .strong()
                        .color(theme::TEXT_MUTED),
                );
                ui.label(
                    RichText::new(
                        "Saved into the open book's Info/index.json. Changes apply on Save.",
                    )
                    .small()
                    .color(theme::TEXT_MUTED),
                );

                ui.add_space(4.0);
                ui.label(
                    RichText::new("manuscript folders (chapter directories)")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                project_changed |= edit_string_list(
                    ui,
                    "settings_manuscript_folders",
                    "Manuscript",
                    &mut book.config.manuscript_folders,
                );

                ui.add_space(8.0);
                ui.label(
                    RichText::new("authors folders (character JSON directories)")
                        .small()
                        .color(theme::TEXT_MUTED),
                );
                project_changed |= edit_string_list(
                    ui,
                    "settings_authors_folders",
                    "Info/Characters",
                    &mut book.config.authors_folders,
                );
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });

            if project_changed {
                app.project_folders_dirty = true;
            }
        });

    if changed {
        app.settings_dirty = true;
    }
    if !open || close {
        if app.settings_dirty {
            let _ = app.settings.save();
            app.settings_dirty = false;
        }
        if app.project_folders_dirty {
            // Persist Info/index.json from the in-memory config the user just
            // edited, then re-open the book so the file tree, manuscript
            // commit, and entity store all pick up the new folder lists.
            if let Some(book_root) = app.book.as_ref().map(|b| b.root.clone()) {
                if let Some(book) = app.book.as_ref() {
                    if let Err(e) = book.config.save(&book.root) {
                        log::warn!("project config save failed: {e}");
                    }
                }
                if let Err(e) = app.open_book(&book_root) {
                    log::warn!("project config reload failed: {e:#}");
                }
            }
            app.project_folders_dirty = false;
        }
        app.show_settings = false;
    }
}

/// Editor for a `Vec<String>` list. Each row is a textbox + `–` button; a
/// trailing row holds an `+ Add` button. Returns `true` whenever the list
/// gained, lost, or had an item edited *and* the textbox lost focus —
/// so callers can defer the persist+reload until the user is done typing.
fn edit_string_list(
    ui: &mut egui::Ui,
    id_salt: &str,
    placeholder: &str,
    list: &mut Vec<String>,
) -> bool {
    let mut changed = false;
    let mut remove: Option<usize> = None;
    for (i, entry) in list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(entry)
                    .id_salt(format!("{id_salt}-{i}"))
                    .hint_text(placeholder)
                    .desired_width(360.0),
            );
            if resp.lost_focus() && resp.changed() {
                changed = true;
            }
            if ui.small_button("–").on_hover_text("Remove").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        list.remove(i);
        changed = true;
    }
    if ui
        .small_button(format!("+ Add {}", placeholder.to_lowercase()))
        .clicked()
    {
        list.push(String::new());
        changed = true;
    }
    changed
}
