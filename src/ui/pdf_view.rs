use crate::app::CkWriterApp;
use crate::pdf::{PageStatus, PdfMeta};
use crate::theme;
use egui::{Color32, ColorImage, RichText, TextureOptions};

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.heading(RichText::new("Read").color(Color32::WHITE));
        ui.separator();
        if app.pdf_building {
            ui.label(RichText::new("● building").color(theme::REVISION_VOICE));
        } else if ui.button("Build PDF").clicked() {
            app.start_pdf_build();
        }
        if let Some(meta) = &app.pdf_meta {
            ui.label(RichText::new(format!("{} pages", meta.page_count)).color(theme::TEXT_MUTED));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new("click any line in the PDF to jump to its source")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        });
    });

    if let Some(err) = app.pdf_error.clone() {
        ui.colored_label(Color32::LIGHT_RED, err);
    }
    ui.separator();

    if app.pdf_meta.is_none() {
        ui.centered_and_justified(|ui| {
            let book_root = app.book.as_ref().map(|b| b.root.clone());
            let pdf_exists = book_root
                .as_ref()
                .map(|r| crate::pdf::pdf_path(r).exists())
                .unwrap_or(false);
            ui.vertical_centered(|ui| {
                if pdf_exists && !app.pdf_building {
                    ui.label(
                        RichText::new("PDF on disk -- open without rebuilding?")
                            .color(theme::TEXT_MUTED),
                    );
                    if ui.button("Open existing PDF").clicked() {
                        app.open_existing_pdf();
                    }
                } else if !app.pdf_building {
                    ui.label(
                        RichText::new("Click Build PDF above to compile with latexmk.")
                            .color(theme::TEXT_MUTED),
                    );
                }
            });
        });
        return;
    }

    let meta = app.pdf_meta.clone().expect("checked above");
    let book_root = match app.book.as_ref().map(|b| b.root.clone()) {
        Some(r) => r,
        None => return,
    };

    let mut clicked: Option<(u32, f32, f32)> = None;

    egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for page in 1..=meta.page_count {
                let status = app
                    .pdf_renderer
                    .as_ref()
                    .map(|r| r.status(page))
                    .unwrap_or(PageStatus::Pending);
                let size = page_size(&meta, &status);
                let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

                if ui.is_rect_visible(rect) {
                    if let Some(r) = app.pdf_renderer.as_mut() {
                        r.request(page);
                    }
                    draw_page(ui, app, page, &status, rect);
                }

                if response.clicked() {
                    if let Some(p) = response.interact_pointer_pos() {
                        let rel = p - rect.min;
                        let dpi = meta.dpi as f32;
                        let x_pt = rel.x * 72.0 / dpi;
                        let y_pt = rel.y * 72.0 / dpi;
                        clicked = Some((page, x_pt, y_pt));
                    }
                }
                ui.add_space(8.0);
            }
        });

    if let Some((page, x_pt, y_pt)) = clicked {
        match crate::pdf::synctex_edit(&book_root, page, x_pt, y_pt) {
            Some(r) => app.jump_to_source(&r.file, r.line),
            None => app.pdf_error = Some("synctex: no source mapping at that point".into()),
        }
    }
}

fn page_size(meta: &PdfMeta, status: &PageStatus) -> egui::Vec2 {
    match status {
        PageStatus::Ready { w, h, .. } => egui::vec2(*w as f32, *h as f32),
        _ => egui::vec2(meta.width_px as f32, meta.height_px as f32),
    }
}

fn draw_page(
    ui: &mut egui::Ui,
    app: &mut CkWriterApp,
    page: u32,
    status: &PageStatus,
    rect: egui::Rect,
) {
    match status {
        PageStatus::Ready { png, .. } => {
            if let std::collections::hash_map::Entry::Vacant(slot) = app.pdf_textures.entry(page) {
                if let Some(t) = load_texture(ui.ctx(), page, png) {
                    slot.insert(t);
                }
            }
            if let Some(t) = app.pdf_textures.get(&page) {
                ui.painter().image(
                    t.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                draw_placeholder(ui, rect, &format!("page {page}"));
            }
        }
        PageStatus::Failed(msg) => {
            ui.painter().rect_filled(rect, 0.0, theme::BG_INSET);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("page {page}: {msg}"),
                egui::FontId::proportional(12.0),
                Color32::LIGHT_RED,
            );
        }
        PageStatus::Pending | PageStatus::Rendering => {
            draw_placeholder(ui, rect, &format!("page {page}…"));
        }
    }
}

fn draw_placeholder(ui: &egui::Ui, rect: egui::Rect, label: &str) {
    ui.painter().rect_filled(rect, 0.0, theme::BG_INSET);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        theme::TEXT_MUTED,
    );
}

fn load_texture(
    ctx: &egui::Context,
    page: u32,
    png: &std::path::Path,
) -> Option<egui::TextureHandle> {
    let bytes = std::fs::read(png).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = rgba.into_raw();
    let color_image = ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(ctx.load_texture(
        format!("page-{page}"),
        color_image,
        TextureOptions::default(),
    ))
}
