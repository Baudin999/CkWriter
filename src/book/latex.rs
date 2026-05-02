use regex::Regex;
use std::sync::OnceLock;

static INCLUDE_RE: OnceLock<Regex> = OnceLock::new();
static CHAPTER_RE: OnceLock<Regex> = OnceLock::new();

fn include_re() -> &'static Regex {
    INCLUDE_RE.get_or_init(|| Regex::new(r"(?m)^[^%\n]*\\include\{([^}]+)\}").unwrap())
}

fn chapter_re() -> &'static Regex {
    CHAPTER_RE.get_or_init(|| Regex::new(r"\\chapter\*?\{([^}]+)\}").unwrap())
}

pub fn parse_includes(main_tex: &str) -> Vec<String> {
    include_re()
        .captures_iter(main_tex)
        .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .collect()
}

pub fn extract_chapter_title(tex: &str) -> Option<String> {
    chapter_re()
        .captures(tex)
        .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
}

/// Strip LaTeX markup down to plain prose for LLM input.
/// - drops comments (`% ...`)
/// - replaces `\emph{X}` with X
/// - replaces `\textit{X}`, `\textbf{X}` with X
/// - removes `\switch`, `\nl`, `\maketitle`
/// - removes `\chapter{X}`, `\section{X}`
///
/// Cursor offsets are not preserved here — this is for sending to the LLM only.
pub fn to_prose(tex: &str) -> String {
    let mut out = String::with_capacity(tex.len());
    for line in tex.lines() {
        // strip line comments (but not escaped %)
        let stripped = strip_line_comment(line);
        out.push_str(&stripped);
        out.push('\n');
    }

    let inline_braced =
        Regex::new(r"\\(?:emph|textit|textbf|texttt|underline)\{([^{}]*)\}").unwrap();
    let mut s = inline_braced.replace_all(&out, "$1").to_string();

    let drops = Regex::new(r"\\(?:switch|nl|maketitle)\b\{?\}?").unwrap();
    s = drops.replace_all(&s, "").to_string();

    let chapter = Regex::new(r"\\(?:chapter|section|subsection)\*?\{([^{}]*)\}").unwrap();
    s = chapter.replace_all(&s, "$1").to_string();

    let begin_end = Regex::new(r"\\(?:begin|end)\{[^{}]*\}").unwrap();
    s = begin_end.replace_all(&s, "").to_string();

    // Collapse triple-blank lines
    let blanks = Regex::new(r"\n{3,}").unwrap();
    s = blanks.replace_all(&s, "\n\n").to_string();
    s
}

fn strip_line_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut prev_backslash = false;
    for ch in line.chars() {
        if ch == '%' && !prev_backslash {
            break;
        }
        out.push(ch);
        prev_backslash = ch == '\\' && !prev_backslash;
    }
    out
}

/// Returns char-index ranges in `text` that are inside a LaTeX command/argument
/// and should NOT be matched as entities (e.g., \emph{irep} — irep is foreign,
/// not a character).
pub fn skip_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();

    // skip line comments
    let mut idx = 0usize;
    let mut prev_backslash = false;
    let mut in_comment = false;
    let mut comment_start = 0usize;
    for ch in text.chars() {
        let len = ch.len_utf8();
        if !in_comment {
            if ch == '%' && !prev_backslash {
                in_comment = true;
                comment_start = idx;
            }
            prev_backslash = ch == '\\' && !prev_backslash;
        } else if ch == '\n' {
            ranges.push((comment_start, idx + len));
            in_comment = false;
            prev_backslash = false;
        }
        idx += len;
    }
    if in_comment {
        ranges.push((comment_start, idx));
    }

    // skip \cmd{...} arguments where the brace pair is on a single match.
    let cmd_arg = Regex::new(r"\\[a-zA-Z]+\*?\{[^{}]*\}").unwrap();
    for m in cmd_arg.find_iter(text) {
        ranges.push((m.start(), m.end()));
    }
    ranges.sort_by_key(|r| r.0);
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_includes() {
        let s = "\\include{Ancient/000_Arrival}\n% \\include{commented}\n\\include{Modern/010}";
        let v = parse_includes(s);
        assert_eq!(v, vec!["Ancient/000_Arrival", "Modern/010"]);
    }

    #[test]
    fn finds_chapter_title() {
        assert_eq!(
            extract_chapter_title("\\chapter{Wua}\nbody"),
            Some("Wua".into())
        );
    }
}
