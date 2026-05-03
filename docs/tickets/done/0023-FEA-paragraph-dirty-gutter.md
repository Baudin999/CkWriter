# 0023 — FEA: Per-paragraph state gutter in the editor

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0002, #0004

## Problem
With #0004 in, the writer can re-run a coach pipeline cheaply — but they have no visual signal of *which* paragraphs are dirty until the run starts and the K/N counter ticks. They want to know, while editing, "if I ran prose right now, which paragraphs would the model see, and does any paragraph have unresolved feedback?" — and ideally without having to read a status line in the side panel. The natural place to put that signal is the editor margin, the way every code editor shows git-diff bars.

A first cut painted a single muted line on any paragraph with a coach cache miss, which lit up every paragraph of a fresh chapter — the opposite of useful. The writer wanted distinguishable states so the gutter answers a richer question without being noisy.

## Scope
- A thin (~3 px) left-margin gutter attached to the LaTeX editor in `src/ui/editor.rs`. Lives at `output.galley_pos.x − GUTTER_GAP_PX − GUTTER_WIDTH_PX/2`, painted with `ui.painter()` after `TextEdit::show` returns so it scrolls with the rendered galley.
- Every paragraph carries a mark; the color encodes one of four states. Priority order — `HasIssues` wins over the parse-status states:
  - **HasIssues (red)** — at least one non-dismissed revision from `{show, prose, spelling}` is anchored in this paragraph (`Revision.paragraph_id == p.id`). Voice excluded.
  - **NeverParsed (muted yellow)** — no cache entry exists in *any* of `last_run_hashes["show, don't tell" / "prose" / "spelling"]` for `p.id`. The model has never seen this paragraph.
  - **Changed (orange)** — at least one of those three labels has an entry for `p.id`, and at least one of the three is either missing or has a hash that no longer matches `p.hash`. The paragraph has been parsed but has drifted.
  - **Clean (gray)** — all three labels have an entry for `p.id` and all match `p.hash`, with zero active issues.
- "Active issues" excludes voice deliberately — voice runs chapter-level, not per-paragraph, so its anchored flags don't belong on the per-paragraph signal.
- States are computed against `current_paragraphs` (the saved index), `current_chapter.meta.last_run_hashes`, and `app.revisions`. The lifecycle store (`book::suggestions`) already filters Accepted/Stale records out of `app.revisions`, and `is_dismissed` mirrors `Status::Dismissed` — so once the writer accepts or dismisses every issue in a paragraph, that paragraph drops back to Clean (or Changed if it's also been edited since the last cache).
- Paragraph-to-pixel conversion uses `output.galley.pos_from_ccursor` for the start char and the char just before the trailing newline. Wrap-aware. Recomputed every editor frame — O(paragraphs × 3) hash lookups + O(revisions) per frame.

## Out of scope
- Click-to-jump from the gutter line to the paragraph. Nice-to-have, not required for this round.
- Live indication during typing for paragraphs that haven't been saved yet. `current_paragraphs` only refreshes on chapter open and on save — the gutter therefore lags live edits until the next save. That's deliberate: the writer gets a clean "since-save" signal, not a flickering per-keystroke one.
- Line numbers, fold markers, or any other gutter content. Keep the gutter single-purpose.
- A separate "git" layer (paragraph changed since HEAD on disk). Distinct semantic from the coach baseline — file-level versioning vs model-level baselines. Worth a follow-up ticket if the writer wants both signals stacked.
- Per-pipeline color coding (three side-by-side dots). The four states already encode the union; per-pipeline detail can live in the AI panel.
- Showing state for any pipeline outside coach (e.g. character-extraction queue, progression). Those are different concerns.

## Acceptance criteria
- [x] On a freshly-opened chapter where no coach has ever run, every paragraph shows the **NeverParsed** (muted yellow) tone — *not* the changed/red palette.
- [x] After running all three of `show`, `prose`, `spelling` to completion on a chapter with no flagged issues, every paragraph shows the **Clean** (gray) tone.
- [x] After running only `prose` (and no other pipeline), every paragraph still shows **Changed** or **NeverParsed** — never **Clean** — because show + spelling are still uncached.
- [x] Editing a paragraph and saving switches that paragraph to **Changed** (orange); the surrounding paragraphs stay **Clean**.
- [x] A paragraph with at least one non-dismissed revision from show/prose/spelling shows **HasIssues** (red), regardless of whether its hash also matches the cache.
- [x] Dismissing or accepting *every* active revision in a previously-red paragraph drops it back to Clean (or Changed if it's also been edited since the last cache).
- [x] Running `voice` (chapter-level) does not change any gutter state, and voice-pipeline revisions never push a paragraph into **HasIssues**.
- [x] The gutter scrolls with the editor — line positions track the rendered galley, not the unwrapped text.
- [x] `cargo clippy --all-targets -- -D warnings` clean; `cargo test` clean.

## Design notes
- **Why four states and not the original one-color binary:** the v1 muted-line treated "never coached" and "changed since coach" identically, which lit up every paragraph of every fresh chapter. Splitting NeverParsed (yellow) from Changed (orange) makes the gutter answer the actual question — "where am I, and what does the model think?" — without conflating "I haven't run the model yet" with "I've edited this since."
- **Why HasIssues takes priority:** the writer's primary use of the gutter is "where do I need to do work?" — a paragraph with unresolved feedback is more important than its parse status. Once feedback is resolved, the parse-status colors take over.
- **Why "since-save" semantics, not "since-keystroke":** `current_paragraphs` is the authoritative paragraph index and only refreshes on save. Recomputing it every keystroke would either duplicate the splitter work in two paths or shift the splitter onto the per-frame editor render. Either is more change than this ticket warrants. The lag is also unlikely to bother the writer — the gutter answers "what will the next coach run cost? what's pending?", a save-time question.
- **Why every paragraph carries a mark:** Clean = quiet gray, not invisible. Trade-off: the gutter is always present (more visual weight than v1's "no mark = clean") but it's also a constant scaffold, so transitions to red/orange/yellow read as state changes against a stable backdrop. Reversible if it ends up feeling busy — drop Clean to invisible and the rest carries the same signal.
- **Performance:** O(paragraphs × 3) hash lookups + O(revisions) per frame. Both N≪chapter; no need to memoize.

## Verification (smoke test)
1. Open a chapter that has never been coached — confirm every paragraph shows the NeverParsed (yellow) tone.
2. Run `prose`. Confirm gutter does *not* go gray (show/spelling still uncached). Mix of NeverParsed/Changed depending on paragraph history.
3. Run `show` and `spelling` to completion with no flagged issues. Confirm every paragraph drops to Clean (gray).
4. Edit one paragraph and save. Confirm only that paragraph turns orange (Changed); the rest stay gray.
5. Re-run `prose` and let it produce at least one flag in some paragraph. Confirm that paragraph turns red (HasIssues), even if other paragraphs are Clean.
6. Dismiss every revision in the red paragraph. Confirm it returns to Clean (or Changed, if also edited since).
7. Run `voice`. Confirm gutter is unaffected — no paragraph turns red because of a voice flag.
