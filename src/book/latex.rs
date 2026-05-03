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

pub const SENTINEL_BEGIN: &str = "% --- BEGIN CKWRITER MANUSCRIPT ---";
pub const SENTINEL_END: &str = "% --- END CKWRITER MANUSCRIPT ---";

/// Returns the include path on a non-comment line, if any. Comment-only lines
/// (`% \include{...}`) are skipped — those are the writer's parking spots.
fn line_include_path(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('%') {
        return None;
    }
    let cap = include_re().captures(line)?;
    cap.get(1).map(|m| m.as_str().trim())
}

/// Rewrite the manuscript include block in `main_tex` with the given ordered
/// list of include paths. The block is delimited by SENTINEL_BEGIN /
/// SENTINEL_END comments; on first run those don't exist, so we wrap the
/// existing run of uncommented `\include{}` lines instead. Lines outside
/// the block (preamble, `\maketitle`, commented parking lines, anything
/// after `\end{document}`) are preserved verbatim.
pub fn sync_main_tex(main_tex: &str, ordered_paths: &[String]) -> String {
    let trailing_newline = main_tex.ends_with('\n');
    let lines: Vec<&str> = main_tex.lines().collect();

    let block = match find_sentinel_block(&lines) {
        Some(Some(span)) => Some(span),
        _ => find_initial_include_block(&lines),
    };

    let new_block = build_block(ordered_paths);

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + new_block.len());
    match block {
        Some((start, end)) => {
            for l in &lines[..start] {
                out.push((*l).to_string());
            }
            out.extend(new_block);
            for l in &lines[end + 1..] {
                out.push((*l).to_string());
            }
        }
        None => {
            // No includes anywhere yet. Insert the block before \end{document}
            // (or at the end if there isn't one).
            let end_doc = lines
                .iter()
                .position(|l| l.trim_start().starts_with("\\end{document}"));
            match end_doc {
                Some(i) => {
                    for l in &lines[..i] {
                        out.push((*l).to_string());
                    }
                    out.extend(new_block);
                    for l in &lines[i..] {
                        out.push((*l).to_string());
                    }
                }
                None => {
                    for l in &lines {
                        out.push((*l).to_string());
                    }
                    out.extend(new_block);
                }
            }
        }
    }

    let mut joined = out.join("\n");
    if trailing_newline {
        joined.push('\n');
    }
    joined
}

fn find_sentinel_block(lines: &[&str]) -> Option<Option<(usize, usize)>> {
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t == SENTINEL_BEGIN {
            start = Some(i);
        } else if t == SENTINEL_END {
            end = Some(i);
        }
    }
    match (start, end) {
        (Some(s), Some(e)) if s <= e => Some(Some((s, e))),
        (Some(_), Some(_)) => Some(None), // sentinels in wrong order — fall back
        _ => None,
    }
}

fn find_initial_include_block(lines: &[&str]) -> Option<(usize, usize)> {
    let mut first: Option<usize> = None;
    let mut last: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if line_include_path(l).is_some() {
            first = first.or(Some(i));
            last = Some(i);
        }
    }
    match (first, last) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

fn build_block(ordered_paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(ordered_paths.len() + 2);
    out.push(SENTINEL_BEGIN.to_string());
    for p in ordered_paths {
        out.push(format!("\\include{{{p}}}"));
    }
    out.push(SENTINEL_END.to_string());
    out
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

    #[test]
    fn first_sync_wraps_existing_block_with_sentinels() {
        let src = "\\documentclass{book}\n\\begin{document}\n\\maketitle\n\
                   \n% \\include{temp}\n\
                   \\include{Ancient/000_Arrival}\n\
                   \\include{Ancient/001_Wua}\n\
                   \n\\end{document}\n";
        let synced = sync_main_tex(
            src,
            &[
                "Ancient/010_Arrival".to_string(),
                "Modern/010_NewYorkCity".to_string(),
                "Ancient/020_Wua".to_string(),
            ],
        );
        assert!(synced.contains(SENTINEL_BEGIN));
        assert!(synced.contains(SENTINEL_END));
        assert!(synced.contains("\\include{Ancient/010_Arrival}"));
        assert!(synced.contains("\\include{Modern/010_NewYorkCity}"));
        assert!(synced.contains("\\include{Ancient/020_Wua}"));
        // Pre-block context survives.
        assert!(synced.contains("\\maketitle"));
        // Parked comment line survives.
        assert!(synced.contains("% \\include{temp}"));
        // Old paths are gone.
        assert!(!synced.contains("\\include{Ancient/000_Arrival}"));
        // \end{document} survives.
        assert!(synced.contains("\\end{document}"));
        // The block's first include comes right after BEGIN.
        let begin_idx = synced.find(SENTINEL_BEGIN).unwrap();
        let arrival_idx = synced.find("\\include{Ancient/010_Arrival}").unwrap();
        assert!(arrival_idx > begin_idx);
    }

    #[test]
    fn re_sync_replaces_only_inside_sentinels() {
        let first = sync_main_tex(
            "\\begin{document}\n\\include{Old/foo}\n\\end{document}\n",
            &["Ancient/010_Arrival".to_string()],
        );
        let again = sync_main_tex(
            &first,
            &[
                "Ancient/010_Arrival".to_string(),
                "Modern/010_NewYorkCity".to_string(),
            ],
        );
        // Idempotent shape.
        assert_eq!(again.matches(SENTINEL_BEGIN).count(), 1);
        assert_eq!(again.matches(SENTINEL_END).count(), 1);
        assert!(again.contains("\\include{Modern/010_NewYorkCity}"));
        assert!(!again.contains("\\include{Old/foo}"));
        assert!(again.contains("\\end{document}"));
    }

    #[test]
    fn empty_main_tex_gets_block_before_end_document() {
        let src = "\\begin{document}\n\\maketitle\n\\end{document}\n";
        let synced = sync_main_tex(src, &["Ancient/010_Arrival".to_string()]);
        let end = synced.find("\\end{document}").unwrap();
        let begin = synced.find(SENTINEL_BEGIN).unwrap();
        assert!(begin < end);
        assert!(synced.contains("\\include{Ancient/010_Arrival}"));
    }

    #[test]
    fn sync_with_zero_paths_leaves_empty_sentinel_block() {
        let src = "\\begin{document}\n\\include{Old/foo}\n\\end{document}\n";
        let synced = sync_main_tex(src, &[]);
        assert!(synced.contains(SENTINEL_BEGIN));
        assert!(synced.contains(SENTINEL_END));
        assert!(!synced.contains("\\include{Old/foo}"));
    }

    #[test]
    fn parses_includes_inside_sentinels() {
        let src = format!(
            "\\begin{{document}}\n{SENTINEL_BEGIN}\n\
             \\include{{Ancient/010_Arrival}}\n\
             \\include{{Modern/010_NewYorkCity}}\n\
             {SENTINEL_END}\n\\end{{document}}\n"
        );
        assert_eq!(
            parse_includes(&src),
            vec!["Ancient/010_Arrival", "Modern/010_NewYorkCity"]
        );
    }
}
