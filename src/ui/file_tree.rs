use crate::app::CkWriterApp;
use crate::book::tree::FileNode;
use crate::book::Chapter;
use crate::theme;
use egui::{Color32, RichText};
use std::collections::HashSet;
use std::path::PathBuf;

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    let Some(book) = &app.book else {
        ui.heading("Project");
        ui.separator();
        ui.label(RichText::new("No book loaded.").color(theme::TEXT_MUTED));
        return;
    };

    ui.heading(book.title());
    ui.separator();

    let children = book.file_tree.children.clone();
    let chapters = book.chapters.clone();
    let current_path = app.current_chapter.as_ref().map(|c| c.file_path.clone());

    let mut to_open: Option<PathBuf> = None;
    let mut to_toggle: Option<PathBuf> = None;

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for node in &children {
                draw_node(
                    node,
                    0,
                    ui,
                    &current_path,
                    &chapters,
                    &app.expanded_dirs,
                    &mut to_open,
                    &mut to_toggle,
                );
            }
        });

    if let Some(p) = to_toggle {
        if !app.expanded_dirs.insert(p.clone()) {
            app.expanded_dirs.remove(&p);
        }
        if let Some(book) = &app.book {
            let entries: Vec<PathBuf> = app.expanded_dirs.iter().cloned().collect();
            app.settings
                .expanded_dirs
                .insert(book.root.clone(), entries);
            app.settings_dirty = true;
        }
    }
    if let Some(p) = to_open {
        app.open_chapter(&p);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_node(
    node: &FileNode,
    depth: usize,
    ui: &mut egui::Ui,
    current: &Option<PathBuf>,
    chapters: &[Chapter],
    expanded: &HashSet<PathBuf>,
    to_open: &mut Option<PathBuf>,
    to_toggle: &mut Option<PathBuf>,
) {
    let is_open = expanded.contains(&node.path);
    let is_current = current.as_ref() == Some(&node.path);

    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 12.0);

        if node.is_dir {
            let chevron = if is_open { "▾" } else { "▸" };
            let label = format!("{chevron}  {}", node.name);
            let text = RichText::new(label).color(theme::ACCENT).strong();
            if ui.selectable_label(false, text).clicked() {
                *to_toggle = Some(node.path.clone());
            }
        } else {
            let chapter = chapters.iter().find(|c| c.file_path == node.path);
            let label = chapter.map(|c| c.display_title.clone()).unwrap_or_else(|| {
                node.path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&node.name)
                    .to_string()
            });

            let mut text = RichText::new(format!("   {label}"));
            match chapter {
                Some(c) if !c.in_manuscript => {
                    text = text.italics().color(theme::TEXT_MUTED);
                }
                Some(_) => {}
                None => {
                    text = text.color(theme::TEXT_MUTED);
                }
            }
            if is_current {
                text = text.color(Color32::WHITE).strong();
            }

            if ui.selectable_label(is_current, text).clicked() {
                *to_open = Some(node.path.clone());
            }
        }
    });

    if node.is_dir && is_open {
        for child in &node.children {
            draw_node(
                child,
                depth + 1,
                ui,
                current,
                chapters,
                expanded,
                to_open,
                to_toggle,
            );
        }
    }
}
