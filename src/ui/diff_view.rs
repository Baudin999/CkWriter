//! Side-by-side diff between the chapter's `HEAD` baseline (left, read-only)
//! and the editor buffer (right, editable). Both columns scroll independently;
//! the diff is recomputed each frame so edits to the right column re-colour
//! immediately.

use crate::app::CkWriterApp;
use crate::diff::{Diff, Span, SpanKind};
use crate::theme;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontFamily, FontId, RichText};

const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 22.0;
const COLUMN_PAD: f32 = 16.0;

pub fn show(app: &mut CkWriterApp, ui: &mut egui::Ui) {
    if app.book.is_none() {
        muted(ui, "Open a book first.");
        return;
    }
    if app.current_chapter.is_none() {
        muted(ui, "Select a chapter to diff against HEAD.");
        return;
    }

    app.ensure_diff_baseline();

    if let Some(err) = &app.diff_baseline_error {
        muted(ui, err);
        return;
    }
    let Some(baseline) = app.diff_baseline.clone() else {
        muted(
            ui,
            "No HEAD baseline for this file (untracked or new chapter).",
        );
        return;
    };

    let d = crate::diff::diff(&baseline, &app.editor_text);

    legend(ui);
    ui.separator();

    let avail_h = ui.available_height();
    // Shared scroll offset: both columns render at this Y, then we read
    // whichever moved (user scrolled it) and propagate that to the other on
    // the next frame. The leading column gets sync within the same frame
    // because we forward its post-input offset to the trailing column.
    let shared = app.diff_scroll_y;
    let (left_y, right_y) = ui.columns(2, |cols| {
        let ly = column_left(&mut cols[0], &baseline, &d, avail_h, shared);
        // If the user scrolled the left column on this frame, hand its
        // new offset to the right column so they paint in lockstep
        // without a one-frame lag.
        let leading = if (ly - shared).abs() > 0.5 {
            ly
        } else {
            shared
        };
        let ry = column_right(&mut cols[1], app, &d, avail_h, leading);
        (ly, ry)
    });
    let next = if (right_y - shared).abs() > (left_y - shared).abs() {
        right_y
    } else {
        left_y
    };
    if (next - app.diff_scroll_y).abs() > 0.5 {
        app.diff_scroll_y = next;
    }
}

fn legend(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("HEAD").color(theme::TEXT_MUTED).small());
        ui.add_space(8.0);
        ui.label(RichText::new("removed").color(theme::DIFF_REMOVED).small());
        ui.label(RichText::new("·").color(theme::TEXT_MUTED));
        ui.label(RichText::new("changed").color(theme::DIFF_CHANGED).small());
        ui.label(RichText::new("·").color(theme::TEXT_MUTED));
        ui.label(RichText::new("new").color(theme::DIFF_INSERTED).small());
        ui.add_space(8.0);
        ui.label(
            RichText::new("right side is editable; ⌘S to save")
                .color(theme::TEXT_MUTED)
                .small(),
        );
    });
}

fn muted(ui: &mut egui::Ui, msg: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(RichText::new(msg).color(theme::TEXT_MUTED));
    });
}

fn column_left(ui: &mut egui::Ui, baseline: &str, d: &Diff, height: f32, scroll_y: f32) -> f32 {
    let mut out_y = scroll_y;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(COLUMN_PAD as i8))
        .show(ui, |ui| {
            ui.set_height(height - 12.0);
            let scroll = egui::ScrollArea::vertical()
                .id_salt("diff-left")
                .auto_shrink([false; 2])
                .vertical_scroll_offset(scroll_y)
                .show(ui, |ui| {
                    let job = build_job(baseline, &d.left);
                    ui.add(egui::Label::new(job).selectable(true));
                });
            out_y = scroll.state.offset.y;
        });
    out_y
}

fn column_right(
    ui: &mut egui::Ui,
    app: &mut CkWriterApp,
    d: &Diff,
    height: f32,
    scroll_y: f32,
) -> f32 {
    let right_spans = d.right.clone();
    let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
        // The text passed in is the live editor buffer. If it's diverged from
        // the spans (race between edit and recompute), fall back to a plain
        // single-format job so we never colour past the buffer's bounds.
        let mut job = if spans_cover(text.len(), &right_spans) {
            build_job(text, &right_spans)
        } else {
            plain_job(text)
        };
        job.wrap.max_width = wrap_width;
        ui.fonts(|f| f.layout_job(job))
    };

    let mut out_y = scroll_y;
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(COLUMN_PAD as i8))
        .show(ui, |ui| {
            ui.set_height(height - 12.0);
            let scroll = egui::ScrollArea::vertical()
                .id_salt("diff-right")
                .auto_shrink([false; 2])
                .vertical_scroll_offset(scroll_y)
                .show(ui, |ui| {
                    let edit = egui::TextEdit::multiline(&mut app.editor_text)
                        .id(egui::Id::new("ckwriter-diff-right"))
                        .font(FontId::new(FONT_SIZE, FontFamily::Monospace))
                        .desired_width(f32::INFINITY)
                        .frame(false)
                        .layouter(&mut layouter);
                    let resp = ui.add(edit);
                    if resp.changed() {
                        app.dirty = true;
                    }
                });
            out_y = scroll.state.offset.y;
        });
    out_y
}

fn spans_cover(len: usize, spans: &[Span]) -> bool {
    if spans.is_empty() {
        return len == 0;
    }
    let mut cursor = 0;
    for s in spans {
        if s.range.0 != cursor {
            return false;
        }
        cursor = s.range.1;
    }
    cursor == len
}

fn build_job(text: &str, spans: &[Span]) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base = TextFormat {
        font_id: FontId::new(FONT_SIZE, FontFamily::Monospace),
        color: theme::TEXT_PRIMARY,
        line_height: Some(LINE_HEIGHT),
        ..Default::default()
    };

    let mut cursor = 0usize;
    for s in spans {
        if s.range.0 < cursor || s.range.1 > text.len() || s.range.0 >= s.range.1 {
            continue;
        }
        if cursor < s.range.0 {
            job.append(&text[cursor..s.range.0], 0.0, base.clone());
        }
        let mut fmt = base.clone();
        let (fg, bg) = colours_for(s.kind);
        fmt.color = fg;
        if let Some(bg) = bg {
            fmt.background = bg;
        }
        job.append(&text[s.range.0..s.range.1], 0.0, fmt);
        cursor = s.range.1;
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, base);
    }
    job
}

fn plain_job(text: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::new(FONT_SIZE, FontFamily::Monospace),
            color: theme::TEXT_PRIMARY,
            line_height: Some(LINE_HEIGHT),
            ..Default::default()
        },
    );
    job
}

/// Foreground + optional background tint for a span. The background tint
/// makes "removed" and "inserted" runs read as gutter-marked rows, which is
/// what makes side-by-side diffs scannable; "changed" is foreground-only so
/// the orange word inside an otherwise-white line still pops.
fn colours_for(kind: SpanKind) -> (Color32, Option<Color32>) {
    match kind {
        SpanKind::Equal => (theme::TEXT_PRIMARY, None),
        SpanKind::Removed => (theme::DIFF_REMOVED, Some(tint(theme::DIFF_REMOVED, 0x22))),
        SpanKind::Inserted => (theme::DIFF_INSERTED, Some(tint(theme::DIFF_INSERTED, 0x22))),
        SpanKind::Changed => (theme::DIFF_CHANGED, Some(tint(theme::DIFF_CHANGED, 0x1c))),
    }
}

fn tint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}
