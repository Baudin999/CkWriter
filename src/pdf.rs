//! PDF building, on-demand per-page rasterization, and SyncTeX edit lookup.
//!
//! Architecture:
//!
//! * Building (`latexmk`) runs on a background thread and returns a `PdfMeta`
//!   over a channel. We do **not** rasterize anything during build; pages are
//!   rendered lazily as they scroll into view.
//! * `PageRenderer` owns a small thread pool of pdftoppm workers. The UI calls
//!   `request(page)` for each visible page; that's idempotent (already-queued
//!   or already-rendered pages are skipped). Workers render one page at a time
//!   with `pdftoppm -singlefile`, cache the PNG on disk keyed by DPI, and post
//!   the resulting `PageStatus` back to the renderer over a channel.
//! * Every external command goes through `subprocess::Guarded`, so a hung
//!   pdftoppm or pdfinfo is killed and reported instead of freezing the UI.

use anyhow::{anyhow, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::subprocess::{GuardOutcome, Guarded};

/// How many pdftoppm workers run in parallel. Four is enough to saturate a
/// typical scroll-into-view burst without spawning a process per page.
const RENDER_WORKERS: usize = 4;

const LATEXMK_TIMEOUT: Duration = Duration::from_secs(180);
const PDFINFO_TIMEOUT: Duration = Duration::from_secs(10);
const PDFTOPPM_TIMEOUT: Duration = Duration::from_secs(60);
const SYNCTEX_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct PdfMeta {
    pub page_count: u32,
    /// First-page width at `dpi`, in pixels. Used as the placeholder size for
    /// pages that haven't rasterized yet so the scroll area's total height is
    /// stable from the moment we open the PDF.
    pub width_px: u32,
    pub height_px: u32,
    pub dpi: u32,
}

#[derive(Clone, Debug)]
pub enum PageStatus {
    Pending,
    Rendering,
    Ready { png: PathBuf, w: u32, h: u32 },
    Failed(String),
}

pub enum BuildOutcome {
    Built(PdfMeta),
    Failed(String),
}

pub fn pdf_path(book_root: &Path) -> PathBuf {
    book_root.join("output").join("main.pdf")
}

pub fn pages_dir(book_root: &Path) -> PathBuf {
    book_root.join("output").join(".ckwriter_pages")
}

fn page_png_path(book_root: &Path, page: u32, dpi: u32) -> PathBuf {
    pages_dir(book_root).join(format!("page-{page}-{dpi}.png"))
}

// -----------------------------------------------------------------------------
// Building
// -----------------------------------------------------------------------------

/// Run `latexmk` in the background, then read PDF metadata. Returns a channel
/// that yields exactly one `BuildOutcome`.
pub fn build_and_meta(book_root: &Path, dpi: u32) -> Receiver<BuildOutcome> {
    spawn_build(book_root.to_path_buf(), dpi, true)
}

/// Skip `latexmk` -- just read metadata for an existing PDF. Used when the
/// user opens Read mode and a PDF is already on disk.
pub fn meta_only(book_root: &Path, dpi: u32) -> Receiver<BuildOutcome> {
    spawn_build(book_root.to_path_buf(), dpi, false)
}

fn spawn_build(book_root: PathBuf, dpi: u32, run_latexmk: bool) -> Receiver<BuildOutcome> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let outcome = if run_latexmk {
            match latexmk(&book_root) {
                Ok(()) => match read_meta(&book_root, dpi) {
                    Ok(m) => BuildOutcome::Built(m),
                    Err(e) => BuildOutcome::Failed(format!("read meta: {e:#}")),
                },
                Err(e) => BuildOutcome::Failed(e),
            }
        } else {
            match read_meta(&book_root, dpi) {
                Ok(m) => BuildOutcome::Built(m),
                Err(e) => BuildOutcome::Failed(format!("read meta: {e:#}")),
            }
        };
        let _ = tx.send(outcome);
    });
    rx
}

fn latexmk(book_root: &Path) -> std::result::Result<(), String> {
    // Defensive: pdflatex won't create subdirs under output/ for \include{Sub/file},
    // so mirror every \include{...} parent dir into output/ first.
    let _ = mirror_include_dirs(book_root);

    let mut cmd = Command::new("latexmk");
    cmd.args([
        "-pdf",
        "-synctex=1",
        "-interaction=nonstopmode",
        "-file-line-error",
        "main.tex",
    ])
    .current_dir(book_root);
    match Guarded::new("latexmk", cmd, LATEXMK_TIMEOUT).run() {
        GuardOutcome::Ok(_) => Ok(()),
        GuardOutcome::NonZero {
            code,
            stdout,
            stderr,
            ..
        } => {
            let combined = format!("{stdout}\n{stderr}");
            Err(format!(
                "latexmk failed (code {code}):\n{}",
                tail_n(&combined, 30)
            ))
        }
        GuardOutcome::TimedOut {
            after,
            partial_stderr,
            ..
        } => Err(format!(
            "latexmk timed out after {after:?} and was killed:\n{}",
            tail_n(&partial_stderr, 30)
        )),
        GuardOutcome::SpawnFailed { error, .. } => Err(format!("spawn latexmk: {error}")),
    }
}

fn mirror_include_dirs(book_root: &Path) -> std::io::Result<()> {
    let main_tex = book_root.join("main.tex");
    let Ok(text) = std::fs::read_to_string(&main_tex) else {
        return Ok(());
    };
    let out_dir = book_root.join("output");
    std::fs::create_dir_all(&out_dir)?;
    let re = regex::Regex::new(r"(?m)^[^%\n]*\\include\{([^}]+)\}").unwrap();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for cap in re.captures_iter(&text) {
        let inc = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if let Some(parent) = Path::new(inc).parent() {
            if parent.as_os_str().is_empty() {
                continue;
            }
            let target = out_dir.join(parent);
            if seen.insert(target.clone()) {
                let _ = std::fs::create_dir_all(&target);
            }
        }
    }
    Ok(())
}

fn tail_n(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= n {
        s.to_string()
    } else {
        lines[lines.len() - n..].join("\n")
    }
}

// -----------------------------------------------------------------------------
// Metadata
// -----------------------------------------------------------------------------

pub fn read_meta(book_root: &Path, dpi: u32) -> Result<PdfMeta> {
    let pdf = pdf_path(book_root);
    if !pdf.exists() {
        return Err(anyhow!("no PDF at {}", pdf.display()));
    }
    let mut cmd = Command::new("pdfinfo");
    cmd.arg(&pdf);
    let stdout = Guarded::new("pdfinfo", cmd, PDFINFO_TIMEOUT)
        .run()
        .into_stdout()?;
    let s = String::from_utf8_lossy(&stdout);

    let mut pages: Option<u32> = None;
    let mut w_pt: Option<f32> = None;
    let mut h_pt: Option<f32> = None;
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("Pages:") {
            pages = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("Page size:") {
            // e.g. "  595.276 x 841.89 pts (A4)"
            let parts: Vec<&str> = v.split_whitespace().collect();
            if parts.len() >= 3 {
                w_pt = parts[0].parse().ok();
                h_pt = parts[2].parse().ok();
            }
        }
    }
    let page_count = pages.ok_or_else(|| anyhow!("pdfinfo did not report Pages"))?;
    // Letter is the safer fallback than A4 for an English-language manuscript;
    // if pdfinfo doesn't report a page size the PDF is malformed anyway.
    let w_pt = w_pt.unwrap_or(612.0);
    let h_pt = h_pt.unwrap_or(792.0);
    let scale = dpi as f32 / 72.0;
    Ok(PdfMeta {
        page_count,
        width_px: (w_pt * scale).round() as u32,
        height_px: (h_pt * scale).round() as u32,
        dpi,
    })
}

// -----------------------------------------------------------------------------
// Per-page rendering
// -----------------------------------------------------------------------------

pub struct PageRenderer {
    statuses: Vec<PageStatus>,
    /// Tracks which pages we've already enqueued or finished, so `request` is
    /// cheap and idempotent across the many frames a page is on screen.
    requested: HashSet<u32>,
    inflight: usize,
    queue_tx: Sender<u32>,
    result_rx: Receiver<(u32, PageStatus)>,
    _workers: Vec<JoinHandle<()>>,
}

impl PageRenderer {
    pub fn new(book_root: &Path, dpi: u32, page_count: u32) -> Self {
        let (queue_tx, queue_rx) = mpsc::channel::<u32>();
        let (result_tx, result_rx) = mpsc::channel::<(u32, PageStatus)>();
        let queue_rx = Arc::new(Mutex::new(queue_rx));

        let mut workers = Vec::with_capacity(RENDER_WORKERS);
        for _ in 0..RENDER_WORKERS {
            let q = Arc::clone(&queue_rx);
            let r = result_tx.clone();
            let root = book_root.to_path_buf();
            workers.push(thread::spawn(move || worker_loop(q, r, root, dpi)));
        }
        // Drop our local result_tx clone; each worker holds its own. When all
        // workers exit, the channel closes and `poll` stops yielding items.
        drop(result_tx);

        Self {
            statuses: vec![PageStatus::Pending; page_count as usize],
            requested: HashSet::new(),
            inflight: 0,
            queue_tx,
            result_rx,
            _workers: workers,
        }
    }

    /// Enqueue a page for rendering. Idempotent: re-requesting a page that's
    /// already pending, rendering, or finished is a no-op.
    pub fn request(&mut self, page: u32) {
        if page == 0 || page as usize > self.statuses.len() {
            return;
        }
        if !self.requested.insert(page) {
            return;
        }
        self.inflight += 1;
        let _ = self.queue_tx.send(page);
    }

    /// Drain all pending status updates from workers. Returns true if anything
    /// changed (UI should repaint).
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok((page, status)) = self.result_rx.try_recv() {
            let idx = (page - 1) as usize;
            if let Some(slot) = self.statuses.get_mut(idx) {
                let was_terminal = matches!(slot, PageStatus::Ready { .. } | PageStatus::Failed(_));
                let is_terminal =
                    matches!(status, PageStatus::Ready { .. } | PageStatus::Failed(_));
                if is_terminal && !was_terminal {
                    self.inflight = self.inflight.saturating_sub(1);
                }
                *slot = status;
                changed = true;
            }
        }
        changed
    }

    pub fn status(&self, page: u32) -> PageStatus {
        if page == 0 {
            return PageStatus::Pending;
        }
        let idx = (page - 1) as usize;
        self.statuses
            .get(idx)
            .cloned()
            .unwrap_or(PageStatus::Pending)
    }

    /// True while any worker is still doing or going to do work. The UI uses
    /// this to keep requesting repaints until everything visible has settled.
    pub fn has_inflight(&self) -> bool {
        self.inflight > 0
    }
}

fn worker_loop(
    queue: Arc<Mutex<Receiver<u32>>>,
    out: Sender<(u32, PageStatus)>,
    book_root: PathBuf,
    dpi: u32,
) {
    loop {
        // Hold the lock only across `recv()` -- once we have the page number we
        // release immediately so other workers can grab the next item.
        let page = {
            let q = queue.lock().expect("render queue poisoned");
            match q.recv() {
                Ok(p) => p,
                Err(_) => return, // queue dropped: renderer is gone, exit.
            }
        };
        if out.send((page, PageStatus::Rendering)).is_err() {
            return;
        }
        let status = render_single_page(&book_root, page, dpi);
        if out.send((page, status)).is_err() {
            return;
        }
    }
}

fn render_single_page(book_root: &Path, page: u32, dpi: u32) -> PageStatus {
    let pdf = pdf_path(book_root);
    if !pdf.exists() {
        return PageStatus::Failed(format!("no PDF at {}", pdf.display()));
    }
    let dest = pages_dir(book_root);
    if let Err(e) = std::fs::create_dir_all(&dest) {
        return PageStatus::Failed(format!("mkdir {}: {e}", dest.display()));
    }
    let target = page_png_path(book_root, page, dpi);

    // Cache hit: a previous session already rendered this page at this DPI.
    if target.exists() {
        match png_dim(&target) {
            Ok((w, h)) => return PageStatus::Ready { png: target, w, h },
            // Corrupt cache file: drop it and re-render below.
            Err(_) => {
                let _ = std::fs::remove_file(&target);
            }
        }
    }

    // pdftoppm -singlefile <prefix> writes <prefix>.png exactly, so we get a
    // deterministic filename without parsing pdftoppm's page-number padding.
    let prefix = target.with_extension("");
    let mut cmd = Command::new("pdftoppm");
    cmd.arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg("-r")
        .arg(dpi.to_string())
        .arg("-png")
        .arg("-singlefile")
        .arg(&pdf)
        .arg(&prefix)
        .stdout(Stdio::null());

    let label = format!("pdftoppm[p={page}]");
    match Guarded::new(label, cmd, PDFTOPPM_TIMEOUT).run() {
        GuardOutcome::Ok(_) => {
            if !target.exists() {
                return PageStatus::Failed(format!(
                    "pdftoppm produced no PNG at {}",
                    target.display()
                ));
            }
            match png_dim(&target) {
                Ok((w, h)) => PageStatus::Ready { png: target, w, h },
                Err(e) => PageStatus::Failed(format!("png dims: {e}")),
            }
        }
        GuardOutcome::TimedOut {
            after,
            partial_stderr,
            ..
        } => PageStatus::Failed(format!(
            "pdftoppm timed out after {after:?} and was killed: {}",
            partial_stderr.trim()
        )),
        GuardOutcome::NonZero { code, stderr, .. } => {
            PageStatus::Failed(format!("pdftoppm exit {code}: {}", stderr.trim()))
        }
        GuardOutcome::SpawnFailed { error, .. } => {
            PageStatus::Failed(format!("spawn pdftoppm: {error}"))
        }
    }
}

fn png_dim(p: &Path) -> Result<(u32, u32)> {
    use image::ImageReader;
    let dims = ImageReader::open(p)?.into_dimensions()?;
    Ok(dims)
}

// -----------------------------------------------------------------------------
// SyncTeX
// -----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SyncTexResult {
    pub file: PathBuf,
    pub line: u32,
}

pub fn synctex_edit(book_root: &Path, page: u32, x_pt: f32, y_pt: f32) -> Option<SyncTexResult> {
    let pdf = pdf_path(book_root);
    let arg = format!("{}:{:.3}:{:.3}:{}", page, x_pt, y_pt, pdf.display());
    let mut cmd = Command::new("synctex");
    cmd.arg("edit").arg("-o").arg(&arg);

    let stdout = match Guarded::new("synctex", cmd, SYNCTEX_TIMEOUT).run() {
        GuardOutcome::Ok(o) => o.stdout,
        _ => return None,
    };
    let s = String::from_utf8_lossy(&stdout);
    let mut file: Option<String> = None;
    let mut line: Option<u32> = None;
    for raw in s.lines() {
        let raw = raw.trim();
        if let Some(v) = raw.strip_prefix("Output:") {
            file = Some(v.trim().to_string());
        } else if let Some(v) = raw.strip_prefix("Line:") {
            line = v.trim().parse().ok();
        }
    }
    let file = file?;
    let line = line?;
    let path = if Path::new(&file).is_absolute() {
        PathBuf::from(file)
    } else {
        book_root.join(file)
    };
    Some(SyncTexResult { file: path, line })
}
