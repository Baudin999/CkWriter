//! Side-by-side diff between an "old" baseline (e.g. `git show HEAD:<path>`)
//! and a "new" buffer (e.g. the editor's working text).
//!
//! Strategy: hybrid LCS. We diff at the line level first, then refine each
//! mixed remove/insert run with a word-level LCS so the user sees an
//! intra-line change as orange instead of a red line + green line.
//!
//! Both LCS passes are O(N*M); to keep memory bounded on pathological inputs
//! (whole-file rewrites, or someone opening a 50k-line file), we cap the
//! table size and fall back to a coarse "everything is removed / inserted"
//! result when exceeded. This is a quality-of-result degradation, not a
//! correctness problem -- the spans still cover the right bytes.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::subprocess::Guarded;

/// One coloured span within either the left (old) or right (new) text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Byte range within the side this span belongs to.
    pub range: (usize, usize),
    pub kind: SpanKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// Identical on both sides; render in normal text colour.
    Equal,
    /// Present only in the old text (left column, red).
    Removed,
    /// Present only in the new text (right column, green).
    Inserted,
    /// Present in both columns but mutated (orange on both sides).
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub left: Vec<Span>,
    pub right: Vec<Span>,
}

/// Above this LCS table size we bail out to a coarse diff. Picked so the
/// table fits comfortably in tens of MB of `u32`s; tune if it ever bites.
const MAX_LCS_CELLS: usize = 4_000_000;

pub fn diff(old: &str, new: &str) -> Diff {
    if old == new {
        let mut d = Diff {
            left: Vec::new(),
            right: Vec::new(),
        };
        if !old.is_empty() {
            d.left.push(Span {
                range: (0, old.len()),
                kind: SpanKind::Equal,
            });
            d.right.push(Span {
                range: (0, new.len()),
                kind: SpanKind::Equal,
            });
        }
        return d;
    }

    let old_lines = split_lines(old);
    let new_lines = split_lines(new);

    let line_ops = match lcs_ops(&old_lines, &new_lines, |a, b| {
        old[a.0..a.1] == new[b.0..b.1]
    }) {
        Some(ops) => ops,
        None => return coarse_diff(old, new),
    };

    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut i = 0;
    while i < line_ops.len() {
        match line_ops[i] {
            RawOp::Equal { o, n } => {
                push_span(&mut left, old_lines[o], SpanKind::Equal);
                push_span(&mut right, new_lines[n], SpanKind::Equal);
                i += 1;
            }
            _ => {
                let start = i;
                while i < line_ops.len()
                    && !matches!(line_ops[i], RawOp::Equal { .. })
                {
                    i += 1;
                }
                refine_block(
                    &line_ops[start..i],
                    &old_lines,
                    &new_lines,
                    old,
                    new,
                    &mut left,
                    &mut right,
                );
            }
        }
    }

    Diff { left, right }
}

fn refine_block(
    ops: &[RawOp],
    old_lines: &[(usize, usize)],
    new_lines: &[(usize, usize)],
    old: &str,
    new: &str,
    left: &mut Vec<Span>,
    right: &mut Vec<Span>,
) {
    let removes: Vec<usize> = ops
        .iter()
        .filter_map(|op| match op {
            RawOp::Remove { o } => Some(*o),
            _ => None,
        })
        .collect();
    let inserts: Vec<usize> = ops
        .iter()
        .filter_map(|op| match op {
            RawOp::Insert { n } => Some(*n),
            _ => None,
        })
        .collect();

    if inserts.is_empty() {
        let r = (
            old_lines[removes[0]].0,
            old_lines[*removes.last().unwrap()].1,
        );
        push_span(left, r, SpanKind::Removed);
        return;
    }
    if removes.is_empty() {
        let r = (
            new_lines[inserts[0]].0,
            new_lines[*inserts.last().unwrap()].1,
        );
        push_span(right, r, SpanKind::Inserted);
        return;
    }

    let old_block = (
        old_lines[removes[0]].0,
        old_lines[*removes.last().unwrap()].1,
    );
    let new_block = (
        new_lines[inserts[0]].0,
        new_lines[*inserts.last().unwrap()].1,
    );

    let old_chunk = &old[old_block.0..old_block.1];
    let new_chunk = &new[new_block.0..new_block.1];

    let old_toks = tokenize(old_chunk);
    let new_toks = tokenize(new_chunk);

    let word_ops = lcs_ops(&old_toks, &new_toks, |a, b| {
        old_chunk[a.0..a.1] == new_chunk[b.0..b.1]
    });

    let Some(word_ops) = word_ops else {
        push_span(left, old_block, SpanKind::Changed);
        push_span(right, new_block, SpanKind::Changed);
        return;
    };

    let mut k = 0;
    while k < word_ops.len() {
        match word_ops[k] {
            RawOp::Equal { o, n } => {
                let lr = (
                    old_block.0 + old_toks[o].0,
                    old_block.0 + old_toks[o].1,
                );
                let rr = (
                    new_block.0 + new_toks[n].0,
                    new_block.0 + new_toks[n].1,
                );
                push_span(left, lr, SpanKind::Equal);
                push_span(right, rr, SpanKind::Equal);
                k += 1;
            }
            _ => {
                let start = k;
                while k < word_ops.len()
                    && !matches!(word_ops[k], RawOp::Equal { .. })
                {
                    k += 1;
                }
                let sub = &word_ops[start..k];
                let r_idx: Vec<usize> = sub
                    .iter()
                    .filter_map(|op| match op {
                        RawOp::Remove { o } => Some(*o),
                        _ => None,
                    })
                    .collect();
                let i_idx: Vec<usize> = sub
                    .iter()
                    .filter_map(|op| match op {
                        RawOp::Insert { n } => Some(*n),
                        _ => None,
                    })
                    .collect();
                if !r_idx.is_empty() && !i_idx.is_empty() {
                    let lr = (
                        old_block.0 + old_toks[r_idx[0]].0,
                        old_block.0 + old_toks[*r_idx.last().unwrap()].1,
                    );
                    let rr = (
                        new_block.0 + new_toks[i_idx[0]].0,
                        new_block.0 + new_toks[*i_idx.last().unwrap()].1,
                    );
                    push_span(left, lr, SpanKind::Changed);
                    push_span(right, rr, SpanKind::Changed);
                } else if !r_idx.is_empty() {
                    let lr = (
                        old_block.0 + old_toks[r_idx[0]].0,
                        old_block.0 + old_toks[*r_idx.last().unwrap()].1,
                    );
                    push_span(left, lr, SpanKind::Removed);
                } else {
                    let rr = (
                        new_block.0 + new_toks[i_idx[0]].0,
                        new_block.0 + new_toks[*i_idx.last().unwrap()].1,
                    );
                    push_span(right, rr, SpanKind::Inserted);
                }
            }
        }
    }
}

/// Coalesce a span into the previous one when both kind and adjacency match.
/// Keeps the output compact; the rendering loop is happier with fewer spans.
fn push_span(out: &mut Vec<Span>, range: (usize, usize), kind: SpanKind) {
    if range.0 >= range.1 {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.kind == kind && last.range.1 == range.0 {
            last.range.1 = range.1;
            return;
        }
    }
    out.push(Span { range, kind });
}

#[derive(Debug, Clone, Copy)]
enum RawOp {
    Equal { o: usize, n: usize },
    Remove { o: usize },
    Insert { n: usize },
}

/// Generic LCS over two index sequences. Returns `None` if the table would
/// exceed `MAX_LCS_CELLS`; the caller is responsible for falling back.
fn lcs_ops<T, F>(a: &[T], b: &[T], eq: F) -> Option<Vec<RawOp>>
where
    F: Fn(&T, &T) -> bool,
{
    let m = a.len();
    let n = b.len();
    if m.checked_mul(n + 1)?.checked_add(n + 1)? > MAX_LCS_CELLS {
        return None;
    }

    let mut tbl = vec![0u32; (m + 1) * (n + 1)];
    let stride = n + 1;
    for i in 0..m {
        for j in 0..n {
            let v = if eq(&a[i], &b[j]) {
                tbl[i * stride + j] + 1
            } else {
                tbl[i * stride + (j + 1)].max(tbl[(i + 1) * stride + j])
            };
            tbl[(i + 1) * stride + (j + 1)] = v;
        }
    }

    let mut ops = Vec::with_capacity(m + n);
    let mut i = m;
    let mut j = n;
    while i > 0 && j > 0 {
        if eq(&a[i - 1], &b[j - 1]) {
            ops.push(RawOp::Equal {
                o: i - 1,
                n: j - 1,
            });
            i -= 1;
            j -= 1;
        } else if tbl[(i - 1) * stride + j] >= tbl[i * stride + (j - 1)] {
            ops.push(RawOp::Remove { o: i - 1 });
            i -= 1;
        } else {
            ops.push(RawOp::Insert { n: j - 1 });
            j -= 1;
        }
    }
    while i > 0 {
        ops.push(RawOp::Remove { o: i - 1 });
        i -= 1;
    }
    while j > 0 {
        ops.push(RawOp::Insert { n: j - 1 });
        j -= 1;
    }
    ops.reverse();
    Some(ops)
}

fn coarse_diff(old: &str, new: &str) -> Diff {
    let mut left = Vec::new();
    let mut right = Vec::new();
    if !old.is_empty() {
        left.push(Span {
            range: (0, old.len()),
            kind: SpanKind::Removed,
        });
    }
    if !new.is_empty() {
        right.push(Span {
            range: (0, new.len()),
            kind: SpanKind::Inserted,
        });
    }
    Diff { left, right }
}

/// Split a string into byte ranges, one per line, keeping the trailing `\n`.
/// A final line without a terminator still gets its own range.
fn split_lines(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut start = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            out.push((start, i + 1));
            start = i + 1;
        }
    }
    if start < bytes.len() {
        out.push((start, bytes.len()));
    }
    out
}

/// Tokenize on word/non-word boundaries. Each output range covers either a
/// run of `is_alphanumeric || '_'` chars or a run of everything else
/// (whitespace, punctuation). Tokens are contiguous and exhaust the input.
fn tokenize(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut iter = s.char_indices().peekable();
    while let Some(&(start, c)) = iter.peek() {
        let want_word = is_word_char(c);
        let mut end = start;
        while let Some(&(idx, ch)) = iter.peek() {
            if is_word_char(ch) != want_word {
                break;
            }
            iter.next();
            end = idx + ch.len_utf8();
        }
        out.push((start, end));
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Load the `HEAD` version of the file at `abs_path` from its enclosing git
/// repository. Returns `Ok(None)` when the file is untracked or there is no
/// repository; returns `Err` when git itself failed.
pub fn head_baseline(abs_path: &Path) -> anyhow::Result<Option<String>> {
    let abs = abs_path.canonicalize()?;
    let dir = abs
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent", abs.display()))?;

    let mut top_cmd = Command::new("git");
    top_cmd
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--show-toplevel");
    let top_out = Guarded::new(
        "git rev-parse --show-toplevel",
        top_cmd,
        Duration::from_secs(2),
    )
    .run();
    let top_bytes = match top_out.into_stdout() {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let top = String::from_utf8(top_bytes)?;
    let top = top.trim();
    if top.is_empty() {
        return Ok(None);
    }
    let top_path = Path::new(top).canonicalize()?;
    let rel = abs.strip_prefix(&top_path)?;
    let rel_str = rel
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-utf8 path: {}", rel.display()))?;

    let mut show_cmd = Command::new("git");
    show_cmd
        .arg("-C")
        .arg(&top_path)
        .arg("show")
        .arg(format!("HEAD:{rel_str}"));
    let show_out = Guarded::new("git show HEAD:<path>", show_cmd, Duration::from_secs(5)).run();
    match show_out.into_stdout() {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        // git exits non-zero for untracked / not-in-HEAD; treat that as "no
        // baseline" rather than an error so the UI can show a friendly state.
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice<'a>(s: &'a str, span: &Span) -> &'a str {
        &s[span.range.0..span.range.1]
    }

    #[test]
    fn identical_inputs_emit_a_single_equal_span() {
        let d = diff("hello world", "hello world");
        assert_eq!(d.left.len(), 1);
        assert_eq!(d.left[0].kind, SpanKind::Equal);
        assert_eq!(d.right[0].kind, SpanKind::Equal);
    }

    #[test]
    fn pure_insert_only_appears_on_the_right() {
        let d = diff("a\n", "a\nb\n");
        assert!(d
            .left
            .iter()
            .all(|s| matches!(s.kind, SpanKind::Equal)));
        let inserts: Vec<&Span> = d
            .right
            .iter()
            .filter(|s| s.kind == SpanKind::Inserted)
            .collect();
        assert_eq!(inserts.len(), 1);
        assert_eq!(slice("a\nb\n", inserts[0]), "b\n");
    }

    #[test]
    fn pure_remove_only_appears_on_the_left() {
        let d = diff("a\nb\n", "a\n");
        let removes: Vec<&Span> = d
            .left
            .iter()
            .filter(|s| s.kind == SpanKind::Removed)
            .collect();
        assert_eq!(removes.len(), 1);
        assert_eq!(slice("a\nb\n", removes[0]), "b\n");
        assert!(d
            .right
            .iter()
            .all(|s| matches!(s.kind, SpanKind::Equal)));
    }

    #[test]
    fn intra_line_edit_emits_change_plus_unchanged_words() {
        // "the cat sat" -> "the dog sat": "cat" -> "dog" should be Changed,
        // "the " and " sat" stay Equal on both sides.
        let old = "the cat sat\n";
        let new = "the dog sat\n";
        let d = diff(old, new);

        let change_l: Vec<&Span> = d
            .left
            .iter()
            .filter(|s| s.kind == SpanKind::Changed)
            .collect();
        let change_r: Vec<&Span> = d
            .right
            .iter()
            .filter(|s| s.kind == SpanKind::Changed)
            .collect();
        assert_eq!(change_l.len(), 1);
        assert_eq!(change_r.len(), 1);
        assert_eq!(slice(old, change_l[0]), "cat");
        assert_eq!(slice(new, change_r[0]), "dog");

        let left_text: String = d.left.iter().map(|s| slice(old, s)).collect();
        let right_text: String = d.right.iter().map(|s| slice(new, s)).collect();
        assert_eq!(left_text, old);
        assert_eq!(right_text, new);
    }

    #[test]
    fn spans_cover_every_byte_of_each_side_in_order() {
        // Property: walking spans in order reconstructs the original text on
        // each side. This is the invariant the renderer leans on.
        let old = "alpha beta\ngamma\ndelta\n";
        let new = "alpha BETA\ngamma\nepsilon\n";
        let d = diff(old, new);

        let mut cursor = 0;
        for s in &d.left {
            assert_eq!(s.range.0, cursor);
            cursor = s.range.1;
        }
        assert_eq!(cursor, old.len());

        let mut cursor = 0;
        for s in &d.right {
            assert_eq!(s.range.0, cursor);
            cursor = s.range.1;
        }
        assert_eq!(cursor, new.len());
    }

    #[test]
    fn empty_old_is_all_insert() {
        let d = diff("", "hello\n");
        assert!(d.left.is_empty());
        assert_eq!(d.right.len(), 1);
        assert_eq!(d.right[0].kind, SpanKind::Inserted);
    }

    #[test]
    fn empty_new_is_all_remove() {
        let d = diff("hello\n", "");
        assert!(d.right.is_empty());
        assert_eq!(d.left.len(), 1);
        assert_eq!(d.left[0].kind, SpanKind::Removed);
    }

    #[test]
    fn utf8_word_change_keeps_byte_boundaries() {
        let old = "café latte\n";
        let new = "café mocha\n";
        let d = diff(old, new);
        for s in d.left.iter().chain(d.right.iter()) {
            assert!(old.is_char_boundary(s.range.0) || new.is_char_boundary(s.range.0));
            assert!(old.is_char_boundary(s.range.1) || new.is_char_boundary(s.range.1));
        }
    }
}
