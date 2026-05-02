use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RawFlag {
    #[serde(default)]
    pub quote: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub suggestion: String,
    /// Spelling-pipeline only: "spelling" | "punctuation" | "grammar".
    /// Empty for voice/show/prose since those have a single category each.
    #[serde(default)]
    pub kind: String,
}

/// Sub-category of a spelling-pipeline flag. Voice/show/prose flags carry
/// `Other`; their colour comes from the pipeline instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    Spelling,
    Punctuation,
    Grammar,
    Other,
}

impl FlagKind {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "spelling" | "spell" => Self::Spelling,
            "punctuation" | "punct" => Self::Punctuation,
            "grammar" | "gram" => Self::Grammar,
            _ => Self::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Spelling => "spelling",
            Self::Punctuation => "punctuation",
            Self::Grammar => "grammar",
            Self::Other => "",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawVoice {
    #[allow(dead_code)]
    #[serde(default)]
    pub score: Option<i32>,
    #[serde(default)]
    pub flags: Vec<RawFlag>,
    #[allow(dead_code)]
    #[serde(default)]
    pub preserved: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RawFlagsOnly {
    #[serde(default)]
    pub flags: Vec<RawFlag>,
}

#[derive(Debug, Clone)]
pub struct Revision {
    pub id: u32,
    pub pipeline: super::prompts::Pipeline,
    pub kind: FlagKind,
    pub quote: String,
    pub why: String,
    pub suggestion: String,
    /// (start, end) byte offsets in the editor buffer; `None` if anchoring failed.
    pub anchor: Option<(usize, usize)>,
}

pub fn parse_voice(buf: &str) -> Option<RawVoice> {
    if let Some(v) = super::parse::parse_json_object::<RawVoice>(buf, "voice") {
        return Some(v);
    }
    let flags: Vec<RawFlag> = super::parse::salvage_array(buf, "flags");
    if flags.is_empty() {
        None
    } else {
        log::warn!(
            "voice: strict parse failed; salvaged {} flag(s) from malformed array",
            flags.len()
        );
        Some(RawVoice { score: None, flags, preserved: Vec::new() })
    }
}

pub fn parse_flags_only(buf: &str) -> Option<RawFlagsOnly> {
    if let Some(v) = super::parse::parse_json_object::<RawFlagsOnly>(buf, "flags") {
        return Some(v);
    }
    let flags: Vec<RawFlag> = super::parse::salvage_array(buf, "flags");
    if flags.is_empty() {
        None
    } else {
        log::warn!(
            "flags: strict parse failed; salvaged {} flag(s) from malformed array",
            flags.len()
        );
        Some(RawFlagsOnly { flags })
    }
}

/// Locate `quote` inside `text`. Returns the first byte-offset match. Quote-anchoring
/// is best-effort: LaTeX prose is stripped before going to the LLM, so a
/// returned quote may not match byte-for-byte. We try:
/// 1. exact substring
/// 2. whitespace-collapsed substring
/// 3. the first 40 chars of the quote as a substring
pub fn anchor(text: &str, quote: &str) -> Option<(usize, usize)> {
    if quote.trim().is_empty() {
        return None;
    }
    if let Some(start) = text.find(quote) {
        return Some((start, start + quote.len()));
    }
    let collapsed_quote = collapse_ws(quote);
    if collapsed_quote.is_empty() {
        return None;
    }
    let collapsed_text = collapse_ws(text);
    if let Some(c_start) = collapsed_text.find(&collapsed_quote) {
        // Map collapsed offset → original offset by walking
        let map = collapse_map(text);
        let c_end = c_start + collapsed_quote.len();
        let start = map.get(c_start).copied()?;
        let end = map.get(c_end).copied().unwrap_or(text.len());
        return Some((start, end));
    }
    let probe_len = quote.chars().take(40).map(|c| c.len_utf8()).sum::<usize>();
    if probe_len >= 8 {
        if let Some(start) = text.find(&quote[..probe_len]) {
            return Some((start, start + probe_len));
        }
    }
    None
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws && !out.is_empty() {
                out.push(' ');
            }
            in_ws = true;
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// For each char-index in the collapsed string, the byte-index in the original.
fn collapse_map(s: &str) -> Vec<usize> {
    let mut map = Vec::with_capacity(s.len());
    let mut in_ws = false;
    let mut byte = 0usize;
    let mut started = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws && started {
                map.push(byte);
            }
            in_ws = true;
        } else {
            map.push(byte);
            in_ws = false;
            started = true;
        }
        byte += ch.len_utf8();
    }
    map.push(byte);
    map
}
