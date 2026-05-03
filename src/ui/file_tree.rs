use crate::app::CkWriterApp;
use crate::book::manuscript::{self, ChapterRef};
use crate::book::tree::FileNode;
use crate::book::Chapter;
use crate::theme;
use egui::{Color32, Frame, Margin, RichText};
use std::collections::HashSet;
use std::path::PathBuf;

/// Top-level tabs in the left sidebar. Manuscript shows the ordered reading
/// list (with parked orphans below); All Files shows the raw on-disk tree
/// for everything else (Info/, top-level notes, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileTreeTab {
    #[default]
    Manuscript,
    AllFiles,
}

/// What the sidebar may want done after a frame. Collected during `show`,
/// applied at the end so the borrow of `app.book` doesn't outlive the closure.
#[derive(Default)]
struct PendingActions {
    open: Option<PathBuf>,
    toggle_dir: Option<PathBuf>,
    reorder: Option<Vec<ChapterRef>>,
    exclude: Option<(String, String)>,
    include: Option<(String, String)>,
    delete_confirm: Option<(String, String)>,
    open_new_chapter: bool,
}

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    let Some(book) = &app.book else {
        ui.heading("Project");
        ui.separator();
        ui.label(RichText::new("No book loaded.").color(theme::TEXT_MUTED));
        return;
    };

    ui.heading(book.title());

    ui.horizontal(|ui| {
        for (tab, label) in [
            (FileTreeTab::Manuscript, "Manuscript"),
            (FileTreeTab::AllFiles, "All Files"),
        ] {
            let selected = app.file_tree_tab == tab;
            if ui.selectable_label(selected, label).clicked() {
                app.file_tree_tab = tab;
            }
        }
    });
    ui.separator();

    let mut pending = PendingActions::default();
    let chapters = book.chapters.clone();
    let manuscript_chapters: Vec<&Chapter> = chapters.iter().filter(|c| c.in_manuscript).collect();
    let orphan_chapters: Vec<&Chapter> = chapters.iter().filter(|c| !c.in_manuscript).collect();
    let current_path = app.current_chapter.as_ref().map(|c| c.file_path.clone());
    let file_tree_children = book.file_tree.children.clone();

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| match app.file_tree_tab {
            FileTreeTab::Manuscript => {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("MANUSCRIPT")
                            .small()
                            .strong()
                            .color(theme::ACCENT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("+ New").clicked() {
                            pending.open_new_chapter = true;
                        }
                    });
                });

                if manuscript_chapters.is_empty() {
                    ui.label(
                        RichText::new("No chapters yet — click \"+ New\".")
                            .small()
                            .color(theme::TEXT_MUTED),
                    );
                } else {
                    draw_manuscript(ui, &manuscript_chapters, &current_path, &mut pending);
                }

                ui.add_space(8.0);

                ui.collapsing(
                    RichText::new(format!("ORPHANS  ({})", orphan_chapters.len()))
                        .small()
                        .strong()
                        .color(theme::TEXT_MUTED),
                    |ui| {
                        if orphan_chapters.is_empty() {
                            ui.label(
                                RichText::new("No parked chapters.")
                                    .small()
                                    .color(theme::TEXT_MUTED),
                            );
                        } else {
                            draw_orphans(ui, &orphan_chapters, &current_path, &mut pending);
                        }
                    },
                );

                if let Some(err) = &app.chapter_op_error {
                    ui.add_space(6.0);
                    ui.colored_label(theme::ERROR, RichText::new(err).small());
                }
            }
            FileTreeTab::AllFiles => {
                for node in &file_tree_children {
                    draw_node(node, 0, ui, &current_path, &app.expanded_dirs, &mut pending);
                }
            }
        });

    apply_pending(app, pending);
}

fn draw_manuscript(
    ui: &mut egui::Ui,
    chapters: &[&Chapter],
    current_path: &Option<PathBuf>,
    pending: &mut PendingActions,
) {
    // Each manuscript row is both a drag source (so it can be picked up) and
    // a drop zone (so dropping on it inserts before that row). A trailing
    // drop zone after the last row handles "drop at end".
    let mut from_idx: Option<usize> = None;
    let mut to_idx: Option<usize> = None;

    for (i, chapter) in chapters.iter().enumerate() {
        let row_id = egui::Id::new(("manuscript-row", i));
        let payload = i;
        let frame = Frame::default().inner_margin(Margin::symmetric(2, 1));
        let (_, dropped) = ui.dnd_drop_zone::<usize, ()>(frame, |ui| {
            ui.dnd_drag_source(row_id, payload, |ui| {
                draw_chapter_row(
                    ui,
                    chapter,
                    current_path,
                    /* in_manuscript */ true,
                    pending,
                );
            });
        });
        if let Some(p) = dropped {
            from_idx = Some(*p);
            to_idx = Some(i);
        }
    }

    // Trailing drop zone — drag a row past the last item to send it to the end.
    let tail_frame = Frame::default().inner_margin(Margin::symmetric(2, 4));
    let (_, dropped) = ui.dnd_drop_zone::<usize, ()>(tail_frame, |ui| {
        ui.label(RichText::new("    ").small().color(theme::TEXT_MUTED));
    });
    if let Some(p) = dropped {
        from_idx = Some(*p);
        to_idx = Some(chapters.len()); // past-the-end → append
    }

    if let (Some(from), Some(mut to)) = (from_idx, to_idx) {
        if from != to {
            // Build the new ChapterRef order.
            let mut new_order: Vec<ChapterRef> = chapters
                .iter()
                .map(|c| ChapterRef {
                    folder: c.folder.clone(),
                    name: c.name.clone(),
                })
                .collect();
            let item = new_order.remove(from);
            // After removing `from`, indices >= from shift down by one. So a
            // "drop at to" target above the removed element keeps its index;
            // a target below it loses one position.
            if to > from {
                to -= 1;
            }
            let to = to.min(new_order.len());
            new_order.insert(to, item);
            pending.reorder = Some(new_order);
        }
    }
}

fn draw_orphans(
    ui: &mut egui::Ui,
    orphans: &[&Chapter],
    current_path: &Option<PathBuf>,
    pending: &mut PendingActions,
) {
    // Group orphans by folder — keeps the two settings visually distinct
    // even when they're parked together at the bottom of the sidebar.
    let mut by_folder: std::collections::BTreeMap<&str, Vec<&Chapter>> =
        std::collections::BTreeMap::new();
    for c in orphans {
        by_folder.entry(c.folder.as_str()).or_default().push(c);
    }

    for (folder, items) in by_folder {
        ui.label(
            RichText::new(folder)
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
        for chapter in items {
            draw_chapter_row(
                ui,
                chapter,
                current_path,
                /* in_manuscript */ false,
                pending,
            );
        }
    }
}

fn draw_chapter_row(
    ui: &mut egui::Ui,
    chapter: &Chapter,
    current_path: &Option<PathBuf>,
    in_manuscript: bool,
    pending: &mut PendingActions,
) {
    let is_current = current_path.as_ref() == Some(&chapter.file_path);
    let badge = if in_manuscript {
        chapter
            .folder
            .chars()
            .next()
            .unwrap_or(' ')
            .to_ascii_uppercase()
    } else {
        '·'
    };
    let label = format!("{badge}  {}", chapter.display_title);
    let mut text = RichText::new(label);
    if !in_manuscript {
        text = text.italics().color(theme::TEXT_MUTED);
    }
    if is_current {
        text = text.color(Color32::WHITE).strong();
    }

    // The selectable_label shows the row's selected state but won't receive
    // clicks: in manuscript rows the dnd_drag_source wrapper above us covers
    // it and egui's hit-test suppresses click-only widgets sitting under a
    // drag-only widget. Opening the chapter goes through the explicit "open"
    // button on the right instead, which sits on top of the drag layer.
    let row_response = ui.horizontal(|ui| {
        // Selectable_label is purely visual here — clicks are intercepted by
        // the dnd_drag_source wrapper above. We keep it for the selected-row
        // background highlight; the open button below is what actually opens.
        let _ = ui.selectable_label(is_current, text);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("open").clicked() {
                pending.open = Some(chapter.file_path.clone());
            }
        });
    });

    row_response.response.context_menu(|ui| {
        if ui.button("Open").clicked() {
            pending.open = Some(chapter.file_path.clone());
            ui.close_menu();
        }
        ui.separator();
        if in_manuscript {
            if ui.button("Exclude from manuscript").clicked() {
                pending.exclude = Some((chapter.folder.clone(), chapter.name.clone()));
                ui.close_menu();
            }
        } else if ui.button("Include in manuscript").clicked() {
            pending.include = Some((chapter.folder.clone(), chapter.name.clone()));
            ui.close_menu();
        }
        ui.separator();
        if ui
            .button(RichText::new("Delete chapter").color(theme::ERROR))
            .clicked()
        {
            pending.delete_confirm = Some((chapter.folder.clone(), chapter.name.clone()));
            ui.close_menu();
        }
    });
}

fn draw_node(
    node: &FileNode,
    depth: usize,
    ui: &mut egui::Ui,
    current: &Option<PathBuf>,
    expanded: &HashSet<PathBuf>,
    pending: &mut PendingActions,
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
                pending.toggle_dir = Some(node.path.clone());
            }
        } else {
            // Show the actual on-disk filename here; this tab is the raw
            // file view, distinct from the prettified Manuscript tab.
            let mut text = RichText::new(format!("   {}", node.name)).color(theme::TEXT_MUTED);
            if is_current {
                text = text.color(Color32::WHITE).strong();
            }

            if ui.selectable_label(is_current, text).clicked() {
                pending.open = Some(node.path.clone());
            }
        }
    });

    if node.is_dir && is_open {
        for child in &node.children {
            draw_node(child, depth + 1, ui, current, expanded, pending);
        }
    }
}

fn apply_pending(app: &mut CkWriterApp, pending: PendingActions) {
    if let Some(p) = pending.toggle_dir {
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
    if pending.open_new_chapter {
        app.new_chapter_dialog = Some(crate::app::NewChapterDialog::default());
    }
    if let Some(new_order) = pending.reorder {
        if let Err(e) = app.reorder_manuscript(new_order) {
            app.chapter_op_error = Some(format!("reorder: {e}"));
        } else {
            app.chapter_op_error = None;
        }
    }
    if let Some((folder, name)) = pending.exclude {
        if let Err(e) = app.exclude_chapter(&folder, &name) {
            app.chapter_op_error = Some(format!("exclude: {e}"));
        } else {
            app.chapter_op_error = None;
        }
    }
    if let Some((folder, name)) = pending.include {
        if let Err(e) = app.include_chapter(&folder, &name) {
            app.chapter_op_error = Some(format!("include: {e}"));
        } else {
            app.chapter_op_error = None;
        }
    }
    if let Some(pair) = pending.delete_confirm {
        app.delete_chapter_confirm = Some(pair);
    }
    if let Some(p) = pending.open {
        app.open_chapter(&p);
    }
    let _ = manuscript::MANAGED_FOLDERS; // keep the import live in release builds
}
