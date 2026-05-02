use crate::app::CkWriterApp;
use crate::book::entity::EntityKind;
use crate::extract;
use crate::theme;
use egui::{Color32, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Characters,
    Locations,
    AI,
    Notes,
}

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for (tab, label) in [
            (Tab::Characters, "Characters"),
            (Tab::Locations, "Locations"),
            (Tab::AI, "AI"),
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
        Tab::Characters => show_kind(app, ui, EntityKind::Character),
        Tab::Locations => show_kind(app, ui, EntityKind::Location),
        Tab::AI => show_ai(app, ui),
        Tab::Notes => show_notes(app, ui),
    }
}

fn show_kind(app: &mut CkWriterApp, ui: &mut egui::Ui, kind: EntityKind) {
    let Some(_book) = &app.book else { return };

    if kind == EntityKind::Character {
        let none_yet = app
            .book
            .as_ref()
            .map(|b| b.entities_of(EntityKind::Character).is_empty())
            .unwrap_or(true);
        if none_yet {
            ui.label(
                RichText::new("No characters yet — seed from Info/Characters/Personae.txt:")
                    .color(theme::TEXT_MUTED),
            );
            if ui.button("Import from Personae.txt").clicked() {
                app.run_import();
            }
            if let Some(s) = &app.import_status {
                ui.label(RichText::new(s).small().color(theme::TEXT_MUTED));
            }
            ui.separator();
        }
    }

    let frequencies = extract::frequency_map(&app.entity_hits);
    let in_scope_ids = extract::by_kind(&frequencies, kind);

    if !in_scope_ids.is_empty() {
        ui.label(RichText::new("In this chapter").small().color(theme::TEXT_MUTED));
        for (id, count) in in_scope_ids {
            entity_row(app, ui, id, Some(count));
        }
        ui.add_space(8.0);
        ui.separator();
    }

    ui.label(RichText::new("All").small().color(theme::TEXT_MUTED));
    let ids: Vec<String> = {
        let book = app.book.as_ref().unwrap();
        book.entities_of(kind).iter().map(|e| e.id.clone()).collect()
    };
    for id in ids {
        entity_row(app, ui, &id, None);
    }

    ui.add_space(12.0);
    if ui.button(format!("+ Add {}", kind_singular(kind))).clicked() {
        app.create_blank_entity(kind);
    }
}

fn kind_singular(k: EntityKind) -> &'static str {
    match k {
        EntityKind::Character => "character",
        EntityKind::Location => "location",
        EntityKind::Event => "event",
        EntityKind::Timeline => "timeline entry",
    }
}

fn entity_row(app: &mut CkWriterApp, ui: &mut egui::Ui, id: &str, count: Option<usize>) {
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

    let pending = app
        .revisions
        .iter()
        .filter(|r| r.status == crate::llm::revision::RevisionStatus::Pending)
        .count();
    ui.label(format!("{pending} pending suggestions"));
    ui.separator();

    let mut accept_id: Option<u32> = None;
    let mut dismiss_id: Option<u32> = None;
    for rev in &app.revisions {
        if rev.status != crate::llm::revision::RevisionStatus::Pending {
            continue;
        }
        let color = match rev.pipeline {
            crate::llm::prompts::Pipeline::Voice => theme::REVISION_VOICE,
            crate::llm::prompts::Pipeline::ShowDontTell => theme::REVISION_SHOW,
            crate::llm::prompts::Pipeline::Prose => theme::REVISION_PROSE,
        };
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(rev.pipeline.label()).color(color).strong());
                if rev.anchor.is_none() {
                    ui.label(RichText::new("(unanchored)").small().color(Color32::LIGHT_RED));
                }
            });
            ui.label(RichText::new(format!("\"{}\"", short(&rev.quote))).italics().color(theme::TEXT_MUTED));
            ui.label(&rev.why);
            if !rev.suggestion.is_empty() {
                ui.label(RichText::new(&rev.suggestion).color(Color32::WHITE));
            }
            ui.horizontal(|ui| {
                if rev.suggestion.is_empty() || rev.anchor.is_none() {
                    ui.add_enabled(false, egui::Button::new("Accept"));
                } else if ui.button("Accept").clicked() {
                    accept_id = Some(rev.id);
                }
                if ui.button("Dismiss").clicked() {
                    dismiss_id = Some(rev.id);
                }
            });
        });
    }
    if let Some(id) = accept_id {
        app.accept_revision(id);
    }
    if let Some(id) = dismiss_id {
        app.dismiss_revision(id);
    }
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
        egui::TextEdit::multiline(&mut app.notes_text).desired_rows(20).hint_text("scratchpad — saved next to the chapter as .notes.md"),
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

fn short(s: &str) -> String {
    let mut t: String = s.chars().take(80).collect();
    if s.chars().count() > 80 {
        t.push('…');
    }
    t
}
