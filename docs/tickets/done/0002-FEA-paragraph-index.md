# 0002 — FEA: Paragraph index inside chapter.json

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0001

## Problem
Coach pipelines today re-prompt the model with the entire chapter on every run, with no notion of paragraph identity. This means every run pays full token cost even if only one paragraph changed, the same paragraph re-generates the same flags (defeating dismissal filtering), and there is no anchor to attach paragraph-level state (locked, last-coached, suggestion history) to. Suggestions can only attach to absolute character offsets, which break under edits.

A stable, content-addressable paragraph index is the substrate that #0003–#0005 hang off.

## Scope

### Splitter
- Module `src/book/paragraphs.rs` with the splitter + matcher.
- Operates on `editor_text` (raw LaTeX), not on `to_prose` output — `char_range` must index into the source so #0005 can map cursor position back to a paragraph.
- A paragraph is a maximal run of non-blank source lines separated by ≥1 blank line.
- `\begin{env}` … `\end{env}` is collapsed into a single paragraph regardless of internal blank lines (no recursion in v1; nested envs treat the outermost only).
- LaTeX command-only lines (e.g. `\section{...}`, `\chapter{...}`) are their own paragraphs.

### Normalization (applied before hashing and fingerprinting only — not before splitting)
- Strip line comments: everything from an unescaped `%` to end-of-line.
- Trim trailing whitespace per line.
- Collapse internal whitespace runs to a single space.
- After normalization, blocks that are empty or comment-only are dropped from the index entirely.
- The original `char_range` still spans the full source block including its comments, so the editor visual indicator in #0005 covers what the writer sees.

### Types
- Persisted (`chapter_meta.rs`, added to `ChapterMeta`):
  ```rust
  pub paragraphs: Vec<ParagraphMeta>   // #[serde(default)]
  pub struct ParagraphMeta { pub id: String, pub hash: String }
  ```
- Runtime (lives on `App`, not on `Chapter`, because it's only meaningful for the open chapter):
  ```rust
  pub current_paragraphs: Vec<Paragraph>
  pub struct Paragraph { pub id: String, pub hash: String, pub char_range: (usize, usize) }
  ```
- Fingerprints (trigram sets used for matching) are computed on demand and **not** persisted. See "Cross-session ID stability" below.

### Hashing
- `hash = hex(blake3(normalized_text))`. blake3 chosen for consistency with #0003.
- Truncated to 16 hex chars (64 bits) — collision-safe at chapter scale and keeps `chapter.json` small.

### ID format and matching
- New ID: `p_` + 8 lowercase hex chars (32 bits — collision-safe within a chapter of <10k paragraphs); regenerate on collision within the chapter.
- Matching algorithm (greedy, deterministic):
  1. For each new paragraph that hash-matches a prior paragraph exactly, assign that ID immediately and remove both from the candidate pools.
  2. For remaining new paragraphs, compute Jaccard similarity (trigram shingles over the normalized text) against every remaining prior paragraph.
  3. Greedy assignment in descending similarity, threshold ≥ 0.5. Each prior paragraph is consumed by the first new paragraph that claims it.
  4. Unmatched new paragraphs get fresh IDs.
- Short-paragraph fallback: if a paragraph normalizes to <8 trigrams (≈10 chars), skip the Jaccard pass for it — only the exact-hash pass can match it. Otherwise tiny paragraphs swap IDs on every keystroke.

### Cross-session ID stability
- Within an open session, the in-memory prior `Vec<Paragraph>` carries fingerprint material across edits, so IDs are stable under the matching rules above.
- Across sessions, only `{id, hash}` is on disk. On reopen, the file content equals what was last saved, so every paragraph hash-matches step 1 trivially → all IDs preserved.
- **Caveat (documented, not fixed in v1):** if the writer edits the `.tex` outside the app between sessions, only paragraphs whose hashes still match exactly will keep their IDs; the rest get fresh IDs. Acceptable trade-off for the byte savings.

### Wiring into the existing flow
- Parse on `seed_chapter_draft` (chapter open): match against `chapter.meta.paragraphs`, write `App::current_paragraphs`. If anything changed (new IDs, etc.), persist the new `Vec<ParagraphMeta>`.
- Parse on `save_chapter`: re-parse `editor_text`, match against `App::current_paragraphs`, persist new `Vec<ParagraphMeta>` via the existing `update_chapter_meta` helper.
- On chapter close / switch: clear `current_paragraphs`.

## Out of scope
- Using paragraph IDs in coach pipelines — that's #0004 (caching).
- Suggestion identity hashing — that's #0003 (lifecycle).
- Locks UI — that's #0005.
- Persisting fingerprints (would help cross-session external edits; deferred until needed).
- Word count is unchanged (still operates on whole-chapter prose).
- Nested-environment splitting.

## Acceptance criteria
- [x] Splitter produces paragraph list matching test fixtures (blank-line splits, env collapse, command-only lines, comment-only blocks dropped, whitespace-only blocks dropped).
- [x] Editing one paragraph's contents preserves IDs of every other paragraph.
- [x] Reordering paragraphs preserves all IDs (matched by content).
- [x] Inserting a new paragraph yields exactly one fresh ID; all others stable.
- [x] Two near-identical paragraphs swapped in order both retain their (now-swapped) IDs — greedy assignment is deterministic and not order-fragile.
- [x] A paragraph normalized to <8 trigrams swaps IDs only on exact-hash mismatch (Jaccard pass skipped).
- [x] `chapter.json` `paragraphs` field round-trips through save/load and is forward-compatible (`#[serde(default)]`).
- [x] Reopening the same chapter without edits preserves every ID (trivial hash-match path).
- [x] `App::current_paragraphs` cleared on chapter switch; populated on `seed_chapter_draft`; refreshed on `save_chapter`.
- [x] `char_range` spans the full source block including its comments and trailing whitespace.
- [x] Fingerprint similarity threshold and short-paragraph cutoff are documented in the module with the rationale above.
- [x] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- Trigram shingles operate on the normalized text after collapsing whitespace, so "the cat sat" and "the  cat\tsat" produce identical trigram sets.
- Greedy descending-similarity assignment is O(N·M) per parse (N new × M prior). Chapters cap at a few hundred paragraphs in practice — fine without a smarter assignment algorithm.
- The persisted `paragraphs` Vec ordering matches `editor_text` order at save time; the matcher does not assume order is meaningful.
- `char_range` is `(usize, usize)` over byte offsets into `editor_text`, half-open `[start, end)`, recomputed on every parse and never persisted (drifts every keystroke).
- The runtime `Vec<Paragraph>` lives on `App`, not on `Chapter`, because the only consumers (#0004 caching, #0005 cursor-to-paragraph mapping) act on the open chapter.
