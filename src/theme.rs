use egui::{Color32, Stroke, Visuals};

pub const BG_PRIMARY: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x22);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x25, 0x25, 0x2a);
pub const BG_INSET: Color32 = Color32::from_rgb(0x18, 0x18, 0x1c);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x90);
pub const ACCENT: Color32 = Color32::from_rgb(0x7a, 0xa2, 0xf7);
pub const ENTITY_CHARACTER: Color32 = Color32::from_rgb(0x9e, 0xce, 0x6a);
pub const ENTITY_LOCATION: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);
pub const REVISION_VOICE: Color32 = Color32::from_rgb(0xf7, 0xc8, 0x6a);
pub const REVISION_SHOW: Color32 = Color32::from_rgb(0x7a, 0xc8, 0xf7);
pub const REVISION_PROSE: Color32 = Color32::from_rgb(0xc8, 0x7a, 0xf7);

pub fn install(ctx: &egui::Context) {
    let mut vis = Visuals::dark();
    vis.window_fill = BG_PRIMARY;
    vis.panel_fill = BG_PANEL;
    vis.extreme_bg_color = BG_INSET;
    vis.override_text_color = Some(TEXT_PRIMARY);
    vis.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x33, 0x33, 0x3a));
    vis.widgets.inactive.bg_fill = BG_INSET;
    vis.widgets.hovered.bg_fill = Color32::from_rgb(0x33, 0x33, 0x3a);
    vis.widgets.active.bg_fill = Color32::from_rgb(0x40, 0x40, 0x48);
    vis.selection.bg_fill = Color32::from_rgba_unmultiplied(0x7a, 0xa2, 0xf7, 0x55);
    vis.selection.stroke = Stroke::new(1.0, ACCENT);
    ctx.set_visuals(vis);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    ctx.set_style(style);
}
