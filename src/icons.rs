//! Font Awesome 4 codepoints. The TTF is embedded in `theme.rs` as a
//! fallback for the Proportional / Monospace / Writer families, so any
//! string containing one of these constants renders the glyph inline.
//!
//! Codepoints are taken from the Font Awesome 4.7.0 cheatsheet
//! (Private Use Area, U+F000–U+F2FF) — they will not collide with prose.

pub const FOLDER: &str = "\u{f114}"; // folder-o
pub const FOLDER_OPEN: &str = "\u{f115}"; // folder-open-o
pub const FILE_TEXT: &str = "\u{f0f6}"; // file-text-o
pub const CHEVRON_DOWN: &str = "\u{f078}";
pub const CHEVRON_RIGHT: &str = "\u{f054}";
pub const PLUS: &str = "\u{f067}";
pub const TIMES: &str = "\u{f00d}";
pub const PENCIL: &str = "\u{f040}";
pub const BOOK: &str = "\u{f02d}";
pub const COG: &str = "\u{f013}";
pub const CIRCLE: &str = "\u{f111}";
pub const CIRCLE_O: &str = "\u{f10c}";
pub const EXCHANGE: &str = "\u{f0ec}";
pub const BARS: &str = "\u{f0c9}";
pub const TRASH: &str = "\u{f1f8}";
pub const PLAY: &str = "\u{f04b}";
