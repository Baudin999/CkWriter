use crate::app::CkWriterApp;
use crate::book::entity::Entity;
use crate::theme;
use egui::RichText;

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    let Some(id) = app.selected_entity.clone() else {
        return;
    };
    let Some(book) = &app.book else { return };
    let Some(existing) = book.entity(&id).cloned() else {
        return;
    };

    ui.heading(&existing.name);
    ui.separator();

    let mut e = existing.clone();
    let mut changed = false;

    egui::Grid::new(format!("inspector-grid-{id}"))
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            field(ui, "name", &mut e.name, &mut changed);
            field(ui, "role", &mut e.role, &mut changed);
            field(ui, "age", &mut e.age, &mut changed);
            field(ui, "tone", &mut e.tone, &mut changed);
        });

    ui.label(RichText::new("voice notes").small().color(theme::TEXT_MUTED));
    if ui
        .add(egui::TextEdit::multiline(&mut e.voice_notes).desired_rows(2).desired_width(f32::INFINITY))
        .changed()
    {
        changed = true;
    }

    ui.label(RichText::new("aliases (comma-separated)").small().color(theme::TEXT_MUTED));
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

    ui.label(RichText::new("free text / lore").small().color(theme::TEXT_MUTED));
    if ui
        .add(egui::TextEdit::multiline(&mut e.free_text).desired_rows(6).desired_width(f32::INFINITY))
        .changed()
    {
        changed = true;
    }

    if changed {
        app.entity_dirty = Some(e);
    }

    ui.add_space(6.0);
    let can_save = app.entity_dirty.is_some();
    ui.horizontal(|ui| {
        if ui.add_enabled(can_save, egui::Button::new("Save")).clicked() {
            app.commit_entity_edit();
        }
        if ui.button("Close").clicked() {
            app.selected_entity = None;
            app.entity_dirty = None;
        }
    });
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, changed: &mut bool) {
    ui.label(label);
    if ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY)).changed() {
        *changed = true;
    }
    ui.end_row();
}

#[allow(dead_code)]
fn _e(_e: &Entity) {}
