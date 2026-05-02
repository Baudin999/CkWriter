use crate::app::CkWriterApp;
use crate::theme;
use egui::{Align, Color32, Layout, RichText};

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let title = app
            .book
            .as_ref()
            .map(|b| b.title().to_string())
            .unwrap_or_else(|| "no book".to_string());
        ui.label(RichText::new(title).strong().size(15.0));

        if let Some(ch) = &app.current_chapter {
            ui.label(RichText::new("·").color(theme::TEXT_MUTED));
            ui.label(RichText::new(&ch.display_title).color(theme::TEXT_MUTED));
        }

        if app.dirty {
            ui.label(RichText::new("●").color(Color32::from_rgb(0xf7, 0xc8, 0x6a)));
        } else {
            ui.label(RichText::new("●").color(theme::TEXT_MUTED));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Settings…").clicked() {
                app.show_settings = !app.show_settings;
            }
            ui.separator();
            if ui.button("Open book…").clicked() {
                app.show_book_picker = true;
            }
            ui.separator();
            let mode_label = if app.read_mode { "Write" } else { "Read" };
            if ui
                .add_enabled(app.book.is_some(), egui::Button::new(mode_label))
                .clicked()
            {
                app.read_mode = !app.read_mode;
                if app.read_mode && app.pdf_meta.is_none() && !app.pdf_building {
                    let pdf = app
                        .book
                        .as_ref()
                        .map(|b| crate::pdf::pdf_path(&b.root).exists())
                        .unwrap_or(false);
                    if pdf {
                        app.open_existing_pdf();
                    }
                }
            }
            ui.separator();
            let dot = if app.ollama_ok {
                RichText::new("●").color(Color32::from_rgb(0x9e, 0xce, 0x6a))
            } else {
                RichText::new("●").color(Color32::from_rgb(0xf7, 0x76, 0x8e))
            };
            ui.label(dot);
            ui.label(RichText::new(&app.settings.model).color(theme::TEXT_MUTED).small());
            ui.label(RichText::new("ollama").color(theme::TEXT_MUTED).small());
        });
    });
}
