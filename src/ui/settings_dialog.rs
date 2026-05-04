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

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });

    if changed {
        app.settings_dirty = true;
    }
    if !open || close {
        if app.settings_dirty {
            let _ = app.settings.save();
            app.settings_dirty = false;
        }
        app.show_settings = false;
    }
}
