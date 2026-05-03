# 0002 — FEA: Paragraph index inside chapter.json

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0001

## Problem
Coach pipelines today re-prompt the model with the entire chapter on every run, with no notion of paragraph identity. This means every run pays full token cost even if only one paragraph changed, the same paragraph re-generates the same flags (defeating dismissal filtering), and there is no anchor to attach paragraph-level state (locked, last-coached, suggestion history) to. Suggestions can only attach to absolute character offsets, which break under edits.

A stable, content-addressable paragraph index is the substrate that #0003–#0005 hang off.

## Scope
- Paragraph splitter: parse `editor_text` LaTeX into source-level paragraphs separated by blank lines. `\begin{...}/\end{...}` environments treated as a single paragraph (no recursion in v1).
- `ParagraphMeta { id: String, hash: String, char_range: (usize, usize) }` at runtime; only `{id, hash}` persisted.
- Persist `paragraphs: Vec<ParagraphMeta>` in `chapter.json` (extends the schema from #0001).
- Stable IDs via best-fingerprint matching: on parse, match new paragraph fingerprints against the previously-saved index. Matched paragraphs keep their existing IDs; unmatched get fresh `p_<8-char-hex>` IDs.
- IDs survive: edits within a paragraph (high content overlap), paragraph reorder, insertion of new paragraphs.
- Module `src/book/paragraphs.rs` with the splitter + matcher.
- Unit tests: blank-line splitting, environment grouping, ID stability across edit/reorder/insert.

## Out of scope
- Using paragraph IDs in coach pipelines — that's #0004 (caching)
- Suggestion identity hashing — that's #0003 (lifecycle)
- Locks UI — that's #0005
- Word count is unchanged (still operates on whole-chapter prose)

## Acceptance criteria
- [ ] Splitter produces paragraph list matching test fixtures
- [ ] Editing one paragraph's contents preserves IDs of every other paragraph
- [ ] Reordering paragraphs preserves all IDs (matched by content)
- [ ] Inserting a new paragraph yields exactly one fresh ID; all others stable
- [ ] `chapter.json` `paragraphs` field round-trips through save/load
- [ ] Fingerprint similarity threshold is documented and justified in the module
- [ ] `cargo clippy` and `cargo test` clean

## Design notes
- Fingerprint: shingle-based (trigram set) with Jaccard similarity, threshold ~0.5. Robust to small edits, separates fundamentally different paragraphs.
- ID format: `p_` + 8 lowercase hex chars (32 bits — collision-safe within a chapter of <10k paragraphs).
- LaTeX command-only paragraphs (e.g. just `\section{...}`) split into their own paragraph for now.
- `char_range` is recomputed on every parse from `editor_text`, never persisted (drifts every keystroke).
