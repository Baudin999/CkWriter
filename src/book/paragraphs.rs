//! Paragraph splitter, hasher, and matcher for the open chapter's LaTeX
//! source. See ticket #0002.
//!
//! ## Splitter
//! - A paragraph is a maximal run of non-blank source lines separated by ≥1
//!   blank line.
//! - `\begin{env} ... \end{env}` collapses into a single paragraph regardless
//!   of internal blank lines. No recursion in v1; nested envs match the next
//!   `\end{name}` literally.
//! - LaTeX command-only lines (e.g. `\section{...}`, `\chapter{...}`) are
//!   their own paragraphs even without surrounding blank lines.
//! - Operates on the raw source so `char_range` (byte offsets, half-open) can
//!   be used by the editor in #0005.
//!
//! ## Normalization (drives hashing + fingerprinting only — never splitting)
//! - Strip line comments: from an unescaped `%` to end-of-line.
//! - Trim trailing whitespace per line.
//! - Collapse internal whitespace runs to a single space.
//! - After normalization, blocks that are empty (or comment-only) are dropped
//!   from the index entirely. The block's `char_range` still covers the
//!   original source — comments included — so the future cursor-to-paragraph
//!   mapping in #0005 lights up the lines the writer actually sees.
//!
//! ## Hashing & matching
//! - `hash = hex(blake3(normalized_text))` truncated to 16 hex chars (64 bits).
//!   blake3 chosen to align with #0003's planned suggestion identity hashing.
//! - ID = `p_` + 8 lowercase hex chars (32 bits). Collision-safe within a
//!   chapter of <10k paragraphs (birthday probability ≈ 1.2%); we still retry
//!   on the in-chapter collision case so it never bites.
//! - Match algorithm (greedy, deterministic):
//!   1. Hash-match (exact). Each prior paragraph is consumed by the first new
//!      paragraph that claims it.
//!   2. Trigram Jaccard on the remaining unmatched paragraphs, threshold
//!      ≥ 0.5, greedy in descending similarity order.
//!   3. Unmatched new paragraphs get fresh ids.
//! - Short-paragraph fallback: a paragraph normalizing to <8 trigrams (≈10
//!   chars) skips the Jaccard pass — only the exact-hash pass can match it.
//!   Without this, tiny paragraphs like a single word would swap ids on every
//!   keystroke as their trigram set fluctuates wildly.
//!
//! ## Cross-session stability
//! Within a session the runtime list (with normalized text) carries fingerprint
//! material across edits. Across sessions only `{id, hash}` is on disk; on
//! reopen the file content equals what was saved, so step 1 trivially matches
//! every paragraph. External edits between sessions break IDs only for the
//! paragraphs whose hashes actually changed — accepted v1 trade-off.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const HASH_HEX_LEN: usize = 16;
const ID_HEX_LEN: usize = 8;
const SIMILARITY_THRESHOLD: f64 = 0.5;
const SHORT_PARAGRAPH_TRIGRAMS: usize = 8;

/// Persisted form: identity + content hash. Lives inside
/// `ChapterMeta::paragraphs`. Ordered by source position at save time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParagraphMeta {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub hash: String,
}

/// Runtime view of a paragraph. `char_range` is byte offsets into the source
/// `editor_text`, half-open `[start, end)`. Recomputed on every parse, never
/// persisted (drifts every keystroke).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    pub id: String,
    pub hash: String,
    pub char_range: (usize, usize),
    /// Normalized content kept in memory so cross-edit Jaccard matching has
    /// fingerprint material to work with. Not persisted — across sessions we
    /// rely on exact hash equality.
    normalized: String,
}

impl Paragraph {
    pub fn meta(&self) -> ParagraphMeta {
        ParagraphMeta {
            id: self.id.clone(),
            hash: self.hash.clone(),
        }
    }
}

/// Parse `text` and match against the in-memory prior list. Returns the new
/// runtime list, ordered by source position.
pub fn parse_and_match(text: &str, prior: &[Paragraph]) -> Vec<Paragraph> {
    let blocks = split_blocks(text);
    let mut new_paragraphs: Vec<Paragraph> = blocks
        .into_iter()
        .map(|b| Paragraph {
            id: String::new(),
            hash: b.hash,
            char_range: b.char_range,
            normalized: b.normalized,
        })
        .collect();
    assign_ids(&mut new_paragraphs, prior);
    new_paragraphs
}

/// Bridge for cross-session reopen: the on-disk index has only `{id, hash}`
/// per entry, so synthesize prior `Paragraph`s with empty `normalized`. Only
/// step 1 (exact hash) can claim ids — adequate because reopen sees the same
/// bytes that were saved.
pub fn parse_and_match_meta(text: &str, prior: &[ParagraphMeta]) -> Vec<Paragraph> {
    let bridged: Vec<Paragraph> = prior
        .iter()
        .map(|m| Paragraph {
            id: m.id.clone(),
            hash: m.hash.clone(),
            char_range: (0, 0),
            normalized: String::new(),
        })
        .collect();
    parse_and_match(text, &bridged)
}

/// True if the persisted index for the chapter would change vs `prior`.
/// Drives the "should we save?" check in `seed_chapter_draft`.
pub fn differs(new: &[Paragraph], prior: &[ParagraphMeta]) -> bool {
    if new.len() != prior.len() {
        return true;
    }
    new.iter()
        .zip(prior.iter())
        .any(|(a, b)| a.id != b.id || a.hash != b.hash)
}

// ============================================================================
// Splitter
// ============================================================================

struct Block {
    char_range: (usize, usize),
    normalized: String,
    hash: String,
}

struct LineSpan {
    start: usize,
    /// Byte offset just past the line's `\n` (or EOF for the final line).
    end: usize,
}

fn line_spans(text: &str) -> Vec<LineSpan> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let end = if i < bytes.len() { i + 1 } else { i };
        out.push(LineSpan { start, end });
        if i < bytes.len() {
            i += 1;
        }
    }
    out
}

fn line_text<'a>(text: &'a str, span: &LineSpan) -> &'a str {
    let raw = &text[span.start..span.end];
    raw.strip_suffix('\n').unwrap_or(raw)
}

fn split_blocks(text: &str) -> Vec<Block> {
    let lines = line_spans(text);
    let mut out: Vec<Block> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let raw = line_text(text, &lines[i]);
        if raw.trim().is_empty() {
            i += 1;
            continue;
        }
        let stripped = strip_line_comment(raw);

        if let Some(env) = parse_begin_env(&stripped) {
            let start = lines[i].start;
            let mut j = i + 1;
            let mut depth = 1usize;
            while j < lines.len() && depth > 0 {
                let l = strip_line_comment(line_text(text, &lines[j]));
                if let Some(other) = parse_begin_env(&l) {
                    if other == env {
                        depth += 1;
                    }
                }
                if has_end_env(&l, &env) {
                    depth = depth.saturating_sub(1);
                }
                j += 1;
            }
            let end_idx = j.saturating_sub(1);
            let end = lines.get(end_idx).map(|s| s.end).unwrap_or(text.len());
            if let Some(b) = make_block(text, start, end) {
                out.push(b);
            }
            i = j;
            continue;
        }

        if is_command_only_line(&stripped) {
            if let Some(b) = make_block(text, lines[i].start, lines[i].end) {
                out.push(b);
            }
            i += 1;
            continue;
        }

        let start = lines[i].start;
        let mut j = i + 1;
        while j < lines.len() {
            let l = line_text(text, &lines[j]);
            if l.trim().is_empty() {
                break;
            }
            let s = strip_line_comment(l);
            if is_command_only_line(&s) {
                break;
            }
            if parse_begin_env(&s).is_some() {
                break;
            }
            j += 1;
        }
        let end = lines[j - 1].end;
        if let Some(b) = make_block(text, start, end) {
            out.push(b);
        }
        i = j;
    }
    out
}

fn make_block(text: &str, start: usize, end: usize) -> Option<Block> {
    let raw = &text[start..end];
    let normalized = normalize(raw);
    if normalized.is_empty() {
        return None;
    }
    let hash = hash_normalized(&normalized);
    Some(Block {
        char_range: (start, end),
        normalized,
        hash,
    })
}

// ============================================================================
// Normalization
// ============================================================================

fn normalize(raw: &str) -> String {
    let mut combined = String::with_capacity(raw.len());
    for line in raw.lines() {
        let stripped = strip_line_comment(line);
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(&stripped);
    }
    let mut out = String::with_capacity(combined.len());
    let mut last_was_space = true;
    for ch in combined.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
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

// ============================================================================
// Structural-line detection
// ============================================================================

fn parse_begin_env(line: &str) -> Option<String> {
    let l = line.trim_start();
    let rest = l.strip_prefix(r"\begin")?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('{')?;
    let close = rest.find('}')?;
    Some(rest[..close].trim().to_string())
}

fn has_end_env(line: &str, name: &str) -> bool {
    let needle = format!(r"\end{{{name}}}");
    line.contains(&needle)
}

fn command_only_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // One or more LaTeX commands (`\foo`, optional `*`, optional bracket
        // options, optional brace args), separated by whitespace, filling the
        // entire line.
        Regex::new(r"^(?:\s*\\[a-zA-Z@]+\*?(?:\[[^\[\]]*\])*(?:\{[^{}]*\})*\s*)+$").unwrap()
    })
}

fn is_command_only_line(stripped: &str) -> bool {
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return false;
    }
    // \begin{env} / \end{env} are env constructs handled separately; treating
    // them as singleton command-only paragraphs would prevent env collapse.
    if trimmed.starts_with(r"\begin{") || trimmed.starts_with(r"\end{") {
        return false;
    }
    command_only_re().is_match(trimmed)
}

// ============================================================================
// Hashing & ID minting
// ============================================================================

fn hash_normalized(s: &str) -> String {
    let h = blake3::hash(s.as_bytes());
    h.to_hex().as_str()[..HASH_HEX_LEN].to_string()
}

/// Derive a fresh id deterministically from the paragraph's normalized text
/// and its position in the chapter, retrying with an incrementing nonce on
/// in-chapter collision.
fn mint_id(normalized: &str, position: usize, taken: &HashSet<String>) -> String {
    let mut nonce: u32 = 0;
    loop {
        let mut h = blake3::Hasher::new();
        h.update(b"ckwriter-paragraph-id-v1");
        h.update(normalized.as_bytes());
        h.update(&(position as u64).to_le_bytes());
        h.update(&nonce.to_le_bytes());
        let hex = h.finalize().to_hex();
        let id = format!("p_{}", &hex.as_str()[..ID_HEX_LEN]);
        if !taken.contains(&id) {
            return id;
        }
        nonce = nonce.wrapping_add(1);
    }
}

// ============================================================================
// Matching
// ============================================================================

fn trigram_set(s: &str) -> HashSet<[char; 3]> {
    let chars: Vec<char> = s.chars().collect();
    let mut set = HashSet::new();
    if chars.len() >= 3 {
        for w in chars.windows(3) {
            set.insert([w[0], w[1], w[2]]);
        }
    }
    set
}

fn jaccard(a: &HashSet<[char; 3]>, b: &HashSet<[char; 3]>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    inter as f64 / union as f64
}

fn assign_ids(new: &mut [Paragraph], prior: &[Paragraph]) {
    let n = new.len();
    let m = prior.len();
    let mut prior_taken = vec![false; m];
    let mut new_assigned = vec![false; n];

    let new_trigrams: Vec<HashSet<[char; 3]>> =
        new.iter().map(|p| trigram_set(&p.normalized)).collect();
    let prior_trigrams: Vec<HashSet<[char; 3]>> =
        prior.iter().map(|p| trigram_set(&p.normalized)).collect();

    // Step 1: exact hash match. Greedy first-fit so two new paragraphs with
    // the same hash claim distinct prior entries when both exist.
    let mut prior_by_hash: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, p) in prior.iter().enumerate() {
        prior_by_hash.entry(p.hash.as_str()).or_default().push(idx);
    }
    for (i, np) in new.iter_mut().enumerate() {
        let Some(list) = prior_by_hash.get_mut(np.hash.as_str()) else {
            continue;
        };
        if let Some(pos) = list.iter().position(|&pi| !prior_taken[pi]) {
            let pi = list[pos];
            np.id = prior[pi].id.clone();
            prior_taken[pi] = true;
            new_assigned[i] = true;
        }
    }

    // Step 2: Jaccard on remaining unmatched paragraphs. Skip both sides if
    // either has fewer than the short-paragraph trigram threshold.
    let mut sims: Vec<(f64, usize, usize)> = Vec::new();
    for i in 0..n {
        if new_assigned[i] {
            continue;
        }
        if new_trigrams[i].len() < SHORT_PARAGRAPH_TRIGRAMS {
            continue;
        }
        for j in 0..m {
            if prior_taken[j] {
                continue;
            }
            if prior_trigrams[j].len() < SHORT_PARAGRAPH_TRIGRAMS {
                continue;
            }
            let sim = jaccard(&new_trigrams[i], &prior_trigrams[j]);
            if sim >= SIMILARITY_THRESHOLD {
                sims.push((sim, i, j));
            }
        }
    }
    sims.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    for (_, i, j) in sims {
        if new_assigned[i] || prior_taken[j] {
            continue;
        }
        new[i].id = prior[j].id.clone();
        prior_taken[j] = true;
        new_assigned[i] = true;
    }

    // Step 3: fresh ids for remaining new paragraphs. Track ids already in
    // play (claimed from prior + freshly minted) so the retry loop catches
    // intra-chapter collisions.
    let mut existing_ids: HashSet<String> = new
        .iter()
        .filter(|p| !p.id.is_empty())
        .map(|p| p.id.clone())
        .collect();
    for i in 0..n {
        if new_assigned[i] {
            continue;
        }
        let id = mint_id(&new[i].normalized, i, &existing_ids);
        existing_ids.insert(id.clone());
        new[i].id = id;
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fresh(text: &str) -> Vec<Paragraph> {
        parse_and_match(text, &[])
    }

    fn ids(ps: &[Paragraph]) -> Vec<String> {
        ps.iter().map(|p| p.id.clone()).collect()
    }

    fn ranges(ps: &[Paragraph]) -> Vec<(usize, usize)> {
        ps.iter().map(|p| p.char_range).collect()
    }

    // --- Splitter ----------------------------------------------------------

    #[test]
    fn blank_line_splits_paragraphs() {
        let src = "Alpha line one.\nAlpha line two.\n\nBeta line.\n";
        let ps = parse_fresh(src);
        assert_eq!(ps.len(), 2);
        assert_eq!(&src[ps[0].char_range.0..ps[0].char_range.1], "Alpha line one.\nAlpha line two.\n");
        assert_eq!(&src[ps[1].char_range.0..ps[1].char_range.1], "Beta line.\n");
    }

    #[test]
    fn env_collapses_internal_blank_lines() {
        let src = "Before.\n\n\\begin{quote}\nA\n\nB\n\\end{quote}\n\nAfter.\n";
        let ps = parse_fresh(src);
        assert_eq!(ps.len(), 3);
        let env_block = &src[ps[1].char_range.0..ps[1].char_range.1];
        assert!(env_block.starts_with("\\begin{quote}"));
        assert!(env_block.contains("\\end{quote}"));
        assert!(env_block.contains("A\n\nB"));
    }

    #[test]
    fn command_only_line_is_its_own_paragraph_without_blank_line() {
        let src = "\\chapter{Awakening}\nFirst sentence of the chapter.\n";
        let ps = parse_fresh(src);
        assert_eq!(ps.len(), 2);
        assert_eq!(&src[ps[0].char_range.0..ps[0].char_range.1], "\\chapter{Awakening}\n");
        assert_eq!(
            &src[ps[1].char_range.0..ps[1].char_range.1],
            "First sentence of the chapter.\n"
        );
    }

    #[test]
    fn comment_only_block_is_dropped() {
        let src = "Real para.\n\n% just a comment\n% second comment\n\nAnother real para.\n";
        let ps = parse_fresh(src);
        assert_eq!(ps.len(), 2);
        let texts: Vec<&str> = ps
            .iter()
            .map(|p| &src[p.char_range.0..p.char_range.1])
            .collect();
        assert_eq!(texts[0], "Real para.\n");
        assert_eq!(texts[1], "Another real para.\n");
    }

    #[test]
    fn whitespace_only_block_is_dropped() {
        // The middle "block" is just whitespace lines (treated as blank), so
        // there is no middle block to drop in the first place — but a block
        // containing only whitespace tokens (e.g. a stray `\` no, leave it
        // simple) confirms the same result.
        let src = "First.\n\n   \n\t\n\nSecond.\n";
        let ps = parse_fresh(src);
        assert_eq!(ps.len(), 2);
        assert_eq!(&src[ps[0].char_range.0..ps[0].char_range.1], "First.\n");
        assert_eq!(&src[ps[1].char_range.0..ps[1].char_range.1], "Second.\n");
    }

    #[test]
    fn char_range_includes_block_internal_comments() {
        let src = "Opens.\n% inline note\nCloses.\n";
        let ps = parse_fresh(src);
        assert_eq!(ps.len(), 1);
        let span = &src[ps[0].char_range.0..ps[0].char_range.1];
        assert!(span.contains("% inline note"));
        assert!(span.starts_with("Opens."));
        assert!(span.ends_with("Closes.\n"));
        // Hash basis must NOT include the comment.
        assert_eq!(ps[0].hash, hash_normalized("Opens. Closes."));
    }

    #[test]
    fn nested_envs_match_outermost_only() {
        let src = "\\begin{outer}\nbody\n\\begin{inner}\nx\n\\end{inner}\nmore\n\\end{outer}\n";
        let ps = parse_fresh(src);
        assert_eq!(ps.len(), 1);
        let span = &src[ps[0].char_range.0..ps[0].char_range.1];
        assert!(span.starts_with("\\begin{outer}"));
        assert!(span.ends_with("\\end{outer}\n"));
    }

    // --- Stability under edits --------------------------------------------

    fn long_para(seed: &str) -> String {
        // Long enough to exceed the trigram threshold.
        format!("{seed} this is a long paragraph with plenty of text to support trigram fingerprinting and avoid the short-paragraph fallback path entirely.")
    }

    #[test]
    fn editing_one_paragraph_preserves_other_ids() {
        let p_a = long_para("Alpha");
        let p_b = long_para("Beta");
        let p_c = long_para("Gamma");
        let v1 = format!("{p_a}\n\n{p_b}\n\n{p_c}\n");
        let first = parse_fresh(&v1);
        assert_eq!(first.len(), 3);

        // Edit the middle paragraph slightly — same skeleton, different word.
        let p_b_edit = long_para("Bravo");
        let v2 = format!("{p_a}\n\n{p_b_edit}\n\n{p_c}\n");
        let second = parse_and_match(&v2, &first);
        assert_eq!(second.len(), 3);
        assert_eq!(second[0].id, first[0].id);
        assert_eq!(second[2].id, first[2].id);
        // Middle id may stay the same (Jaccard similar) or change (if not).
        // The acceptance criterion is just that the OTHER ids are stable.
    }

    #[test]
    fn reordering_preserves_all_ids() {
        let p_a = long_para("Alpha");
        let p_b = long_para("Beta");
        let p_c = long_para("Gamma");
        let v1 = format!("{p_a}\n\n{p_b}\n\n{p_c}\n");
        let first = parse_fresh(&v1);
        let v2 = format!("{p_c}\n\n{p_a}\n\n{p_b}\n");
        let second = parse_and_match(&v2, &first);
        assert_eq!(second.len(), 3);
        assert_eq!(second[0].id, first[2].id);
        assert_eq!(second[1].id, first[0].id);
        assert_eq!(second[2].id, first[1].id);
    }

    #[test]
    fn inserting_a_paragraph_yields_one_fresh_id() {
        let p_a = long_para("Alpha");
        let p_b = long_para("Beta");
        let v1 = format!("{p_a}\n\n{p_b}\n");
        let first = parse_fresh(&v1);
        let p_new = long_para("Inserted");
        let v2 = format!("{p_a}\n\n{p_new}\n\n{p_b}\n");
        let second = parse_and_match(&v2, &first);
        assert_eq!(second.len(), 3);
        assert_eq!(second[0].id, first[0].id);
        assert_eq!(second[2].id, first[1].id);
        let prior_ids: HashSet<String> = first.iter().map(|p| p.id.clone()).collect();
        assert!(!prior_ids.contains(&second[1].id), "inserted paragraph reused a prior id");
    }

    #[test]
    fn near_identical_swap_retains_swapped_ids() {
        // Two near-identical long paragraphs that differ in one trailing
        // word; swapped, then matched. Step-1 hash match fails (hashes
        // differ), so the Jaccard pass has to do the work.
        let base = "the protagonist crossed the bridge under heavy mist while the river churned beneath rotten timbers and the wind kept rising";
        let a = format!("{base} alpha-suffix.");
        let b = format!("{base} beta-suffix.");
        let v1 = format!("{a}\n\n{b}\n");
        let first = parse_fresh(&v1);
        assert_eq!(first.len(), 2);
        let id_a = first[0].id.clone();
        let id_b = first[1].id.clone();
        assert_ne!(id_a, id_b);

        // Swap the two — and edit each one slightly so step 1 cannot fire.
        let a_edit = format!("{base} alpha-suffix changed.");
        let b_edit = format!("{base} beta-suffix changed.");
        let v2 = format!("{b_edit}\n\n{a_edit}\n");
        let second = parse_and_match(&v2, &first);
        assert_eq!(second.len(), 2);
        // The new[0] is closer to prior[1] (b), so it inherits id_b.
        assert_eq!(second[0].id, id_b);
        assert_eq!(second[1].id, id_a);
    }

    #[test]
    fn short_paragraph_skips_jaccard() {
        // A single-word paragraph normalizes to <8 trigrams. Editing one
        // character yields a different hash, no Jaccard match, fresh id.
        let v1 = "Alpha\n";
        let first = parse_fresh(v1);
        assert_eq!(first.len(), 1);
        let id1 = first[0].id.clone();

        let v2 = "Alphb\n";
        let second = parse_and_match(v2, &first);
        assert_eq!(second.len(), 1);
        assert_ne!(second[0].id, id1);
    }

    #[test]
    fn reopening_unchanged_chapter_preserves_every_id() {
        let p_a = long_para("Alpha");
        let p_b = long_para("Beta");
        let p_c = long_para("Gamma");
        let v = format!("{p_a}\n\n{p_b}\n\n{p_c}\n");
        let first = parse_fresh(&v);
        let meta: Vec<ParagraphMeta> = first.iter().map(|p| p.meta()).collect();

        // Reopen path: only `{id, hash}` survives.
        let reloaded = parse_and_match_meta(&v, &meta);
        assert_eq!(reloaded.len(), first.len());
        for (a, b) in reloaded.iter().zip(first.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.hash, b.hash);
        }
        assert!(!differs(&reloaded, &meta));
    }

    #[test]
    fn paragraph_meta_round_trips_through_serde() {
        let p_a = long_para("Alpha");
        let p_b = long_para("Beta");
        let v = format!("{p_a}\n\n{p_b}\n");
        let parsed = parse_fresh(&v);
        let meta: Vec<ParagraphMeta> = parsed.iter().map(|p| p.meta()).collect();
        let json = serde_json::to_string(&meta).unwrap();
        let back: Vec<ParagraphMeta> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn ids_have_expected_shape() {
        let p_a = long_para("Alpha");
        let parsed = parse_fresh(&format!("{p_a}\n"));
        assert_eq!(parsed.len(), 1);
        let id = &parsed[0].id;
        assert!(id.starts_with("p_"));
        assert_eq!(id.len(), 2 + ID_HEX_LEN);
        assert!(id[2..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn hash_truncated_to_64_bits() {
        let h = hash_normalized("anything");
        assert_eq!(h.len(), HASH_HEX_LEN);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn differs_detects_length_change() {
        let parsed = parse_fresh("first.\n\nsecond.\n");
        let meta: Vec<ParagraphMeta> = parsed.iter().take(1).map(|p| p.meta()).collect();
        assert!(differs(&parsed, &meta));
    }

    #[test]
    fn differs_detects_hash_change() {
        let parsed = parse_fresh("first.\n");
        let mut meta: Vec<ParagraphMeta> = parsed.iter().map(|p| p.meta()).collect();
        meta[0].hash = "deadbeefdeadbeef".into();
        assert!(differs(&parsed, &meta));
    }

    #[test]
    fn ranges_are_contiguous_and_in_order() {
        let src = "First para line one.\nFirst line two.\n\n\\section{Mid}\n\nLast paragraph here.\n";
        let ps = parse_fresh(src);
        let rs = ranges(&ps);
        for i in 1..rs.len() {
            assert!(rs[i].0 >= rs[i - 1].1, "ranges overlap or out of order: {rs:?}");
        }
        // All ranges are within bounds and start ≤ end.
        for r in &rs {
            assert!(r.0 <= r.1);
            assert!(r.1 <= src.len());
        }
    }

    #[test]
    fn ids_are_unique_within_chapter() {
        // Two identical paragraphs should still receive distinct ids.
        let p = long_para("Twin");
        let src = format!("{p}\n\n{p}\n");
        let ps = parse_fresh(&src);
        assert_eq!(ps.len(), 2);
        assert_ne!(ps[0].id, ps[1].id);
        // And both hash the same.
        assert_eq!(ps[0].hash, ps[1].hash);
    }

    #[test]
    fn duplicate_hashes_match_one_to_one_to_prior() {
        // Two identical new paragraphs against two identical prior — each
        // claims one prior id, none repeats.
        let p = long_para("Twin");
        let src = format!("{p}\n\n{p}\n");
        let first = parse_fresh(&src);
        let second = parse_and_match(&src, &first);
        let mut firsts = ids(&first);
        let mut seconds = ids(&second);
        firsts.sort();
        seconds.sort();
        assert_eq!(firsts, seconds);
    }

    // --- Normalization spot-checks ----------------------------------------

    #[test]
    fn normalize_collapses_whitespace_and_strips_comments() {
        let raw = "  the cat\tsat\n   on % oh wait\nthe mat   \n";
        // Lines: "  the cat\tsat", "   on ", "the mat   "
        // After per-line strip + join " " + collapse: "the cat sat on the mat"
        assert_eq!(normalize(raw), "the cat sat on the mat");
    }

    #[test]
    fn normalize_handles_escaped_percent() {
        let raw = "give me 50\\% of the loot\n";
        assert_eq!(normalize(raw), "give me 50\\% of the loot");
    }

    #[test]
    fn command_only_recognises_compound_commands() {
        assert!(is_command_only_line(r"\section{Foo}"));
        assert!(is_command_only_line(r"\section*{Foo}"));
        assert!(is_command_only_line(r"\chapter{Awakening}\label{ch:awake}"));
        assert!(is_command_only_line(r"\maketitle"));
        assert!(!is_command_only_line(r"\section{Foo} trailing prose"));
        assert!(!is_command_only_line(r"\begin{quote}"));
        assert!(!is_command_only_line(""));
    }
}
