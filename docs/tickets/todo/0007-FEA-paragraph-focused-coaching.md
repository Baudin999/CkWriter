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
- Distinct from existing `start_single_paragraph_run` (#0024 play button): that path sends only the paragraph as prose. Focus mode sends the full chapter as prose with a system-prompt directive scoping output. Two different modes; do not collapse.

## Implementation plan

Build in two phases. Phase 1 is the contract (testable without UI); phase 2 wires it up.

### Phase 1 — contract layer
1. **`src/llm/prompts.rs`** — add `FocusContext { paragraph_id: String, paragraph_text: String }`. Extend `build_system` with optional `focus: Option<&FocusContext>`. When `Some`, append a directive after the pipeline instructions: full chapter is for context, only emit flags whose `quote` is an exact substring of `<paragraph_text>`. Off-target flags will be dropped server-side. `focus = None` must produce byte-identical output to today's `build_system` (regression test).
2. **`src/app/coach.rs`** — `CoachRun` gains `focus: Option<FocusParagraph>` where `FocusParagraph { id, text }`. New entry point `run_pipeline_focus(pipeline, paragraph_id)` parallel to `run_pipeline`: refuses if paragraph is locked, builds a one-entry queue containing the *full chapter prose* (mirror `start_paragraph_run` chapter-mode path, not `start_single_paragraph_run`), threads `focus` through to prompt construction. Bypasses cache read; cache write still lands for the focus paragraph only.
3. **Ingest filter** — at the point where parsed flags become `Suggestion`s, when `focus.is_some()`, drop any flag whose `quote` is not a substring of `focus.text`. Log each drop: `WARN focus={paragraph_id} dropped off-target flag: <quote-truncated>`.
4. **Voice score guard** — when `focus.is_some()` on a `Pipeline::Voice` run, skip the chapter score update.
5. **Logging** — extend the existing pipeline-start log line with `focus=<paragraph_id>` (or `focus=none`) so prompt construction is auditable. `prose_chars` already covers AC #3.

### Phase 2 — UI
6. **Cursor → paragraph_id** helper in `src/ui/editor.rs` (or wherever the editor exposes its cursor): walk `current_paragraphs[].char_range` for the cursor offset. Returns `Option<&Paragraph>`.
7. **Pipeline buttons** — paired "this paragraph" button next to each of the four pipeline buttons. Disabled when cursor isn't inside a paragraph or paragraph is locked. Click → `run_pipeline_focus`.

### Tests (contract proof)
- `prompts::tests::build_system_focus_appends_directive_with_paragraph_text`
- `prompts::tests::build_system_no_focus_byte_identical_to_legacy`
- `coach::tests::focus_run_uses_full_chapter_prose`
- `coach::tests::focus_run_drops_off_target_flag`
- `coach::tests::focus_run_keeps_on_target_flag`
- `coach::tests::focus_voice_run_does_not_update_score`
- `coach::tests::focus_run_refuses_locked_paragraph`

## Status notes (parked 2026-05-03)
Implementation plan above is complete and unblocked. Parked because the writer's actual blocker is the LaTeX editing surface, not coach noise — recent ticket sequence (#0004 cache, #0025 history, #0027 notes, #0005 locks) all addresses coach noise without addressing the surface friction that is keeping the writer from putting words on the page. Resume after #0015 (LaTeX highlighting), #0028 (Super+I emph wrap), and #0029 (Super+L diff balloon) ship and we re-evaluate whether per-pipeline focus is still wanted. Nothing in the codebase has been changed for #0007 yet — no rollback needed.
