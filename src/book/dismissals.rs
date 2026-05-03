//! Quote-normalization shared between the legacy `coach-dismissals.json`
//! migrator and the per-chapter suggestion lifecycle (#0003). Lower-cased,
//! whitespace-collapsed, trimmed: minor model variation in quote selection
//! (extra space, capitalization, curly vs straight apostrophe) still matches
//! a previously-seen quote.

pub const LEGACY_FILE_NAME: &str = "Info/coach-dismissals.json";

/// Lowercase, collapse runs of whitespace to a single space, trim. Keeps
/// punctuation as-is — punctuation differences are usually meaningful to the
/// flag (e.g. a missing comma is the whole point of a spelling flag).
pub fn normalize(quote: &str) -> String {
    let mut out = String::with_capacity(quote.len());
    let mut last_was_space = false;
    for ch in quote.chars() {
        if ch.is_whitespace() {
            if !last_was_space && !out.is_empty() {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            last_was_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace_and_lowercases() {
        assert_eq!(normalize("  Hello   World  "), "hello world");
        assert_eq!(normalize("Tab\there"), "tab here");
        assert_eq!(normalize("LINE\nBREAK"), "line break");
    }

    #[test]
    fn normalize_preserves_punctuation() {
        assert_eq!(normalize("It's, you know."), "it's, you know.");
    }
}
