use crate::app::CkWriterApp;
use crate::theme;
use egui::{Color32, RichText};
use std::path::PathBuf;

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    ui.heading("Chapters");
    ui.separator();

    let Some(book) = &app.book else {
        ui.label(RichText::new("No book loaded.").color(theme::TEXT_MUTED));
        return;
    };

    let chapters = book.chapters.clone();
    let mut to_open: Option<PathBuf> = None;

    let mut last_group = String::new();
    for ch in &chapters {
        if ch.group != last_group {
            ui.add_space(4.0);
            ui.label(RichText::new(&ch.group).strong().color(theme::ACCENT));
            last_group = ch.group.clone();
        }
        let is_current = app
            .current_chapter
            .as_ref()
            .map(|c| c.file_path == ch.file_path)
            .unwrap_or(false);

        let mut text = RichText::new(&ch.display_title);
        if !ch.in_manuscript {
            text = text.italics().color(theme::TEXT_MUTED);
        }
        if is_current {
            text = text.color(Color32::WHITE).strong();
        }
        if ui.selectable_label(is_current, text).clicked() {
            to_open = Some(ch.file_path.clone());
        }
    }

    if let Some(p) = to_open {
        app.open_chapter(&p);
    }
}
