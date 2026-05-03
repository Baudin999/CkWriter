# 0007 — FEA: Paragraph-focused coaching mode

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0002

## Problem
The four coach pipelines (voice/show/prose/spelling) always run against the whole chapter. Mid-revision, the writer often wants advice only on the paragraph they're actively working on — not 30 flags scattered across paragraphs they haven't touched. Today the only way to focus is mental: ignore everything outside the current paragraph in the panel, which is noise the writer has to filter by hand every run.

The whole chapter still needs to be in the model's context so voice consistency, callbacks, and prose-rhythm comparisons land — but the *output* should be scoped to the cursor's paragraph.

## Scope
- Pipeline runner gains a "focus paragraph" mode: full chapter prose still goes in, system prompt instructs the model to only emit flags whose `quote` falls inside paragraph `<paragraph_id>`'s text.
- Cursor → paragraph_id resolution: walk `current_paragraphs[].char_range` for the cursor offset.
- New UI affordance: when the cursor is inside a paragraph, the four pipeline buttons gain a "this paragraph" toggle (or a paired button row) that runs the same pipeline scoped.
- Ingest validates that returned flags actually anchor inside the focus paragraph; off-target flags are dropped with a log warning.
- Locked paragraphs (#0005) are not eligible — button disabled or hidden when cursor is on a locked paragraph.
- Token usage and prompt-construction logged per run, marked `focus=true`.

## Out of scope
- Multi-paragraph focus (a span / selection-based scope)
- Auto-focus heuristics ("just run on every paragraph that changed since last run") — that's closer to #0004
- A separate "focus" pipeline kind — focus is an option on the existing pipelines, not a fifth one

## Acceptance criteria
- [ ] With the cursor inside a paragraph, pressing a pipeline's "this paragraph" button runs the pipeline and produces flags only for that paragraph
- [ ] Off-target flags from the model are dropped with a log line (no panel pollution)
- [ ] The full chapter is still in the prompt — verify by inspecting the logged `prose_chars`
- [ ] Voice score is *not* updated when running in focus mode (the score is chapter-level)
- [ ] Locked paragraph (#0005) → focus run for that paragraph is disabled
- [ ] `cargo clippy` and `cargo test` clean

## Design notes
- Why pass the full chapter for a single-paragraph ask? Voice and show-don't-tell flags depend on chapter-level pacing and tone; cutting context to one paragraph degrades quality. The cost is unchanged context tokens; the win is fewer output flags and tighter focus.
- Why not a selection-based scope in v1? Paragraph_id is already the substrate; selections add an anchor-resolution layer with no extra value for the cursor-on-paragraph workflow. Re-file as a follow-up if it comes up.
- Future interaction with #0004: paragraph-focused runs should bypass per-paragraph cache (the writer is asking for a fresh take on this paragraph specifically). Cache-write still happens.
