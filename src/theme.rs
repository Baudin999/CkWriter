use egui::{Color32, FontData, FontDefinitions, FontFamily, Stroke, Visuals};
use std::sync::Arc;

pub const BG_PRIMARY: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x22);
pub const BG_PANEL: Color32 = Color32::from_rgb(0x25, 0x25, 0x2a);
pub const BG_INSET: Color32 = Color32::from_rgb(0x18, 0x18, 0x1c);
pub const EDITOR_PAGE: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x20);
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xe6);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x90);
pub const ACCENT: Color32 = Color32::from_rgb(0x7a, 0xa2, 0xf7);
pub const ERROR: Color32 = Color32::from_rgb(0xf7, 0x6a, 0x6a);
pub const ENTITY_CHARACTER: Color32 = Color32::from_rgb(0x9e, 0xce, 0x6a);
pub const ENTITY_LOCATION: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);
pub const REVISION_VOICE: Color32 = Color32::from_rgb(0xf7, 0xc8, 0x6a);
pub const REVISION_SHOW: Color32 = Color32::from_rgb(0x7a, 0xc8, 0xf7);
pub const REVISION_PROSE: Color32 = Color32::from_rgb(0xc8, 0x7a, 0xf7);
pub const REVISION_SPELLING: Color32 = Color32::from_rgb(0xf7, 0x6a, 0x6a);
pub const REVISION_PUNCTUATION: Color32 = Color32::from_rgb(0xf7, 0xa8, 0x6a);
pub const REVISION_GRAMMAR: Color32 = Color32::from_rgb(0xa8, 0xc8, 0x7a);
pub const REVISION_SELECTED_BG: Color32 = Color32::from_rgb(0x33, 0x2c, 0x2c);

// --- Editor per-paragraph state gutter (#0023) ----------------------------
// Four states with priority HasIssues > NeverParsed | Changed > Clean. Tones
// chosen to be distinguishable on the dark editor page (`EDITOR_PAGE`) while
// staying calm enough to read as ambient state, not alert. The hue ordering
// (yellow → orange → red) intentionally mirrors familiar status conventions
// so the writer can scan the margin at a glance.
/// Paragraph the model has never seen.
pub const GUTTER_NEVER_PARSED: Color32 = Color32::from_rgb(0xc8, 0xa8, 0x55);
/// Paragraph that was parsed, then edited since.
pub const GUTTER_CHANGED: Color32 = Color32::from_rgb(0xd0, 0x88, 0x4a);
/// Paragraph carries one or more unresolved revisions from the per-paragraph
/// pipelines (show/prose/spelling). Highest-priority gutter color.
pub const GUTTER_HAS_ISSUES: Color32 = Color32::from_rgb(0xd0, 0x55, 0x55);
/// Paragraph parsed by all three per-paragraph pipelines, no drift, no
/// active issues. The quiet baseline tone.
pub const GUTTER_CLEAN: Color32 = Color32::from_rgb(0x55, 0x55, 0x5d);

pub const DIFF_REMOVED: Color32 = Color32::from_rgb(0xf7, 0x76, 0x8e);
pub const DIFF_INSERTED: Color32 = Color32::from_rgb(0x9e, 0xce, 0x6a);
pub const DIFF_CHANGED: Color32 = Color32::from_rgb(0xe0, 0xaf, 0x68);

pub const WRITER_FAMILY: &str = "writer";

const IA_WRITER_DIR: &str = "/usr/share/fonts/ttf-ia-writer";

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);

    let mut vis = Visuals::dark();
    vis.window_fill = BG_PRIMARY;
    vis.panel_fill = BG_PANEL;
    vis.extreme_bg_color = EDITOR_PAGE;
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

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut writer_chain: Vec<String> = Vec::new();

    if let Some(bytes) = read_font(&format!("{IA_WRITER_DIR}/iAWriterQuattroS-Regular.ttf")) {
        fonts.font_data.insert(
            "ia-writer-quattro".to_owned(),
            Arc::new(FontData::from_owned(bytes)),
        );
        writer_chain.push("ia-writer-quattro".to_owned());
    } else {
        log::warn!(
            "iA Writer Quattro S not found at {IA_WRITER_DIR}; falling back to default proportional font"
        );
    }

    // Always fall back through the built-in proportional font so the family
    // resolves even if iA Writer isn't installed on this machine.
    writer_chain.push("Ubuntu-Light".to_owned());

    // Bundle Font Awesome 4 so icon glyphs (PUA, U+F000–U+F2FF) render
    // anywhere we drop them into a string. Loaded as a fallback on every
    // family rather than a separate FontFamily so callers don't need to
    // switch fonts mid-layout.
    let fa_bytes = include_bytes!("../assets/fonts/fontawesome-webfont.ttf");
    fonts.font_data.insert(
        "fontawesome".to_owned(),
        Arc::new(FontData::from_static(fa_bytes)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("fontawesome".to_owned());
    }
    writer_chain.push("fontawesome".to_owned());

    fonts
        .families
        .insert(FontFamily::Name(WRITER_FAMILY.into()), writer_chain);

    ctx.set_fonts(fonts);
}

fn read_font(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}
