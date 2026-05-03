# 0022 — REFACTOR: Adopt ropey + per-paragraph render units

**Type:** REFACTOR
**Created:** 2026-05-03
**Depends on:** none

## Problem
The manuscript editor renders the entire chapter as **one** `egui::TextEdit::multiline` over a single `String` (`src/app/mod.rs:76`, `src/ui/editor.rs:170-178`). Every keystroke triggers a global re-layout of the whole chapter, with all decorations (entity hits, revision underlines, future inline annotations) recomputed against absolute byte offsets into one giant buffer. This is the wrong shape for a writing tool, and it is the **structural** cause of two ongoing bug classes:

1. **Highlight flicker on type (#0017).** The "fix" landed a pre-render `refresh_entity_hits` gate (`editor.rs:61-64`), but at the top of `show()` the keystroke for that frame has not yet been applied — egui mutates `editor_text` only inside `edit.show(ui)`. The layouter therefore still receives the new text against a closure-captured snapshot of the pre-keystroke hits. The strobe described in #0017 is one frame late, not eliminated. The unit tests in `editor.rs:591-703` only cover the fingerprint function; nothing exercises the typing-frame timing.
2. **Anchor drift / off-by-N positioning (#0021 and similar).** Decorations carry absolute `(byte_start, byte_end)` into `editor_text`. Any edit above an anchored span shifts every span below it. The whole document is one address space, so any cross-paragraph mutation is a positional foot-gun.

Beyond bugs, the single-buffer model blocks several near-term initiatives:
- **Per-paragraph caching (#0004)** wants paragraph-stable identity for cache keys.
- **Paragraph locks (#0005)** wants per-paragraph state.
- **Paragraph-focused coaching (#0007)** wants to scope LLM context to one paragraph.
- **Three-layer memory (#0008)** and **always-loaded context (#0011)** want paragraph-granular recall.
- **Generous undo** (writers expect Scrivener-class history) requires cheap snapshots; cloning the full `String` per undo point is wasteful and gets worse as chapters grow.

The cost of *not* doing this: every new paragraph-scoped feature pays a tax of "first, find this paragraph in `editor_text` and translate offsets," #0017-class flicker bugs continue to surface in new decoration layers (LaTeX command highlighting #0015, dyslexia-friendly reading #0020), and undo gets shoe-horned in on top of a buffer that doesn't structurally support it.

### Why ropey, not `Vec<String>`
A `Vec<String>` of paragraphs is a degenerate one-level rope. Once we add paragraph splits/joins on Enter/Backspace, byte arithmetic across the chapter, undo snapshots, cross-paragraph selection, and find/replace, we are reinventing ropey badly — without its B-tree, without structural sharing for snapshots, without standardised char/byte/line iteration. The decisive factor is undo: ropey snapshots are O(diff), `Vec<String>` snapshots are O(chapter). For a writing tool with hundreds of restore points, that difference is real.

### Why per-paragraph render units
egui's `TextEdit` is the unit of layout. As long as the chapter is one TextEdit, a keystroke anywhere reflows everything, and decorations get recomputed against a moving target. With per-paragraph TextEdits:
- Paragraph N's text and paragraph N's decorations share one render unit; they cannot desync.
- Editing paragraph 5 does not invalidate paragraphs 1-4's galleys.
- Decorations are paragraph-local — anchors are `(paragraph_id, byte_in_paragraph)`, not absolute byte offsets that drift with every edit elsewhere.
- The flicker class collapses: there is no "global text mutated mid-frame against captured global hits" race because there is no global text.

These two changes are independent in principle but coupled in practice: per-paragraph render needs paragraph-stable storage, and the rope is the cleanest backing for that.

## Scope
This ticket replaces the single-buffer, single-widget editor with a paragraph-addressed document model and a multi-widget render. Everything below is in scope.

### Storage: rope as source of truth
- Add `ropey` as a dependency.
- Introduce `Document` (proposed module: `src/book/document.rs`) owning a `ropey::Rope` plus a derived paragraph index. Exposes:
  - `paragraphs() -> &[ParagraphView]` where `ParagraphView` carries `paragraph_id`, byte range in the rope, and a cached `String` materialisation when needed by a render unit.
  - `replace_paragraph(id, new_text)` / `split_paragraph(id, at)` / `join_paragraphs(a, b)` — the only mutation surface. All cursor/selection/edit operations route through these.
  - `slice(paragraph_id) -> RopeSlice` and `whole_text() -> String` for export / find-replace / LLM prompts.
  - Snapshot + restore primitives sized for undo (clone the rope; structural sharing keeps it cheap).
- Replace `CkWriterApp::editor_text: String` (`src/app/mod.rs:76`) with `editor_doc: Option<Document>`. Keep a single transitional accessor `editor_text(&self) -> Cow<'_, str>` that materialises the whole rope, used **only** by code that legitimately needs the full chapter (save, export, LLM prompts, find-replace). Audit every existing reader and convert paragraph-scoped readers off the accessor.
- `current_paragraphs: Vec<Paragraph>` (`src/app/mod.rs:87`) becomes a view derived from `Document`, not an independently maintained list. The paragraph-id scheme from #0002 carries forward; ids must remain stable across save/load.

### Render: per-paragraph widgets
- Rewrite `src/ui/editor.rs::show` to render one `egui::TextEdit::multiline` per paragraph, stacked inside the existing `ScrollArea`.
- Each paragraph widget gets a stable `egui::Id::new(("ckwriter-paragraph", paragraph_id))` so focus, selection, and TextEditState survive frames.
- Each paragraph maintains its own `String` buffer materialised from the rope on first render or on rope-side change. On `response.changed()`, write the buffer back into the rope via `Document::replace_paragraph`. Materialisation is lazy and bounded to visible paragraphs (egui's clipping plus a "kept-alive recently focused" set).
- The layouter for each paragraph is a closure that decorates **only** that paragraph's text with **only** that paragraph's hits/revisions. The fingerprint cache from #0017 fix #2 stays per-paragraph and now actually buys us idle-frame skipping without racing against typing.
- Cross-paragraph cursor movement (Up/Down at top/bottom edge, Home/End across, PageUp/PageDown, click-through) is implemented in a `DocumentEditor` controller layer that observes per-paragraph `TextEditOutput`s and transfers focus + places a `CCursor` on the destination paragraph. Selection across paragraphs is tracked at the controller level as `(paragraph_id, byte)` start/end pairs and rendered by each paragraph widget in its own coordinate space.

### Decorations: paragraph-local addressing
- `EntityHit` keeps its byte offsets but they now index a **paragraph slice**, not the whole chapter. The matcher runs per paragraph (`extract::EntityMatcher::find` is already pure-functional, so this is a call-site change). `App::entity_hits: Vec<EntityHit>` becomes paragraph-keyed: `HashMap<ParagraphId, Vec<EntityHit>>` (or stored on `ParagraphView`).
- Revisions get `anchor: Option<(ParagraphId, usize, usize)>` instead of `(usize, usize)` into the global buffer. `revision::anchor` and the rebuild path (`coach.rs:407-466`) anchor against a paragraph slice. The store (`SuggestionRecord`) keeps the raw quote text as today; the anchor is recomputed on rebuild.
- Anchor drift bug (#0021) becomes structurally impossible for paragraph-local anchors: an edit in paragraph 3 cannot shift paragraph 7's offsets.

### Undo / redo
- Introduce a snapshot ring on `Document`: each significant edit (paragraph boundary, focus loss, periodic checkpoint — final policy in design notes) clones the rope into the ring. Bounded depth (proposed: 200 entries; tunable).
- Cmd+Z / Cmd+Shift+Z restore the most recent / next snapshot, replace the active document, refresh the paragraph index, and re-anchor revisions/hits.
- Snapshots store the rope (cheap via structural sharing) plus the cursor position `(paragraph_id, byte)` so undo restores the writer to where they were, not where they last saved.

### Migration of consumers
Every site that touches `editor_text` today has to be reviewed. Inventory (non-exhaustive — verify before starting):
- `src/ui/editor.rs` — full rewrite (this ticket).
- `src/app/book.rs` — chapter open/save/resync paths (`open_chapter`, `delete_chapter`, `resync_current_chapter`), `refresh_entity_hits`, `jump_to_anchor`. All become `Document`-aware.
- `src/app/coach.rs` — ingest path (`coach.rs:234-238`), rebuild path (`coach.rs:407-466`), auto-stale (`run_auto_stale`). Anchors become paragraph-relative.
- `src/extract.rs` — `find` is pure; call sites change, function unchanged.
- LLM prompt assembly — wherever the chat/coach pipelines stuff `editor_text` into a system prompt, switch to `document.whole_text()` (or a paragraph slice for paragraph-focused coaching #0007).
- Persistence — save/load round-trips through the rope. The on-disk file format is unchanged (it's already `\n\n`-separated paragraphs); only the in-memory representation changes.

### Quality gates (per CLAUDE.md, non-negotiable)
- `cargo clippy --all-targets -- -D warnings` returns 0 warnings, 0 errors.
- `cargo test` returns 0 failures.
- New tests at three layers:
  - `Document` unit tests: split/join/replace, paragraph-id stability across edits, snapshot restore, byte arithmetic at paragraph boundaries.
  - Decoration re-anchoring tests: edit in paragraph N does not move anchors in paragraphs ≠ N.
  - Editor integration test (headless egui or scripted input where feasible): a simulated keystroke does not produce a frame where any paragraph's decoration spans index outside that paragraph's text.
- The flicker described in #0017 is verified absent by manual smoke test on a chapter with several entity highlights and several spelling revisions, typing inside and adjacent to highlighted spans.

## Out of scope
- **New undo UX** beyond Cmd+Z / Cmd+Shift+Z (no history panel, no named restore points). The snapshot ring is the foundation; the surface is minimal.
- **Find/replace UI.** `Document` exposes the slicing primitives that find/replace will need, but the writer-facing find dialog is its own ticket.
- **Per-paragraph caching (#0004), paragraph locks (#0005), paragraph-focused coaching (#0007).** Those tickets become *easier* after this lands; they are not folded in.
- **LaTeX-aware paragraph splitting.** Paragraphs remain `\n\n`-delimited as today. Smarter boundaries (sentence-level, environment-aware) are future work.
- **Replacing `egui::TextEdit` with a custom widget.** We continue to use TextEdit per paragraph; this ticket does not write a from-scratch text input.
- **Rope-backed in-place editing of the active widget.** Each paragraph still materialises into a `String` for its TextEdit. A future optimisation could write a TextEdit variant that edits a `RopeSlice` directly; that is not required for the flicker fix or the architectural goals here.
- **Migrating settings / chat / form text inputs to ropey.** Those are short single-line fields; `String` is correct for them.

## Acceptance criteria
- [ ] `editor_text: String` is gone from `CkWriterApp`. The replacement `Document` is the single source of truth for chapter text.
- [ ] The manuscript editor renders one `TextEdit` per paragraph; typing in paragraph N does not cause paragraphs ≠ N to be re-laid-out (verified via the existing `build_job` debug-trace counter, which fires per-paragraph and shows zero rebuilds in non-edited paragraphs during a typing burst).
- [ ] Highlight flicker (#0017) is absent: typing inside or adjacent to an entity-highlighted name produces no visible strobe on the colour or underline. Same for spelling-revision underlines.
- [ ] Anchor drift bug class is closed: a regression test edits paragraph 3 of a multi-paragraph chapter and asserts that revision anchors in paragraph 7 still index the same substring before and after.
- [ ] Cursor movement across paragraph boundaries (arrow keys at edges, click into a different paragraph, Home/End/PageUp/PageDown) behaves indistinguishably from the current single-TextEdit experience for a writer; selection across paragraphs is supported.
- [ ] Cmd+Z restores the rope, paragraph index, cursor position, and visible decorations to the previous snapshot. Cmd+Shift+Z redoes. Snapshot ring depth is configurable; default ≥ 100.
- [ ] Save/load round-trips the chapter byte-for-byte for an arbitrary input — the rope-backed in-memory model produces the same on-disk file format as today.
- [ ] Paragraph identity (#0002) is stable across edits within the same session and across save/load. Adding/removing paragraphs above paragraph N does not change paragraph N's id.
- [ ] LLM pipelines (coach, chat, character extraction) receive identical input strings to today: the migration is invisible to prompt assembly.
- [ ] `cargo clippy --all-targets -- -D warnings` returns 0 warnings, 0 errors. `cargo test` returns 0 failures.

## Design notes

### Why ropey specifically
- Already battle-tested in the Rust ecosystem (used by Helix, lapce, Zed's predecessors). Mature API, well-documented gotchas around char vs byte indices.
- B-tree gives O(log n) for the operations we'll actually do across the whole chapter: byte index → paragraph, slice, length.
- Structural sharing on `clone()` makes the undo-snapshot ring cheap. This is the strongest argument for ropey over `Vec<String>` and the one that decided this ticket.
- Single dependency, no transitive surprises. License is MIT.

### Bridging ropey ↔ egui::TextEdit
egui's `TextEdit::multiline` requires `&mut String`, not `&mut Rope`. The chosen pattern:
- Each visible paragraph holds a `materialised: String` synced from the rope.
- On `response.changed()`, the new `materialised` is written back via `Document::replace_paragraph(id, materialised.clone())`. This rebuilds that paragraph's slot in the rope and updates the paragraph index for the paragraph(s) affected.
- Materialisations are bounded: ~visible paragraphs + a small recently-focused window. A chapter with 500 paragraphs does not materialise 500 strings — egui's clipping already gives us ~30 visible at a time.
- Off-screen paragraphs drop their materialisation when they leave the kept-alive set; they re-materialise from the rope when scrolled back into view.

This is the "bridge" cost of using ropey with egui. It is real but bounded; not worth writing a custom widget that edits `RopeSlice` directly until / unless profiling shows the materialisation churn matters.

### Paragraph identity scheme
- New paragraphs (split via Enter) get a fresh id from a monotonic counter on `Document`. Splitting paragraph 5 into 5 + 5a: paragraph 5 keeps its id; the new tail gets a new id.
- Joining (Backspace at start of paragraph) merges right-into-left: the left paragraph's id wins; the right paragraph's id is retired.
- Save/load: ids are persisted alongside paragraph text in whatever the persistence format becomes. Today the on-disk format is plain text with `\n\n` boundaries — ids are reassigned on load in source order. **Open question for refinement:** do we need cross-session id stability for any consumer? If yes, persist ids; if no, source-order assignment is fine. Decorations stored on `SuggestionRecord` re-anchor by quote text, not id, so they are unaffected either way.

### Undo snapshot policy
Proposed default:
- Snapshot on focus change between paragraphs (writer paused on this paragraph; commit the edit).
- Snapshot on a 2-second idle timer within a paragraph (long pause within the paragraph).
- Snapshot on save.
- Snapshot before any structural mutation (paragraph split/join, AI accept-suggestion).
- Bounded ring of 200 snapshots; oldest dropped on overflow.

Per-keystroke snapshots are too granular and would surprise writers. Per-save-only is too coarse (writers expect intra-session undo). The pause-based heuristic mirrors how Word and Google Docs cluster edits.

### Ordering hint for the implementer (not a sub-ticket boundary)
The ticket is not split, but a sane working order avoids long broken-build periods:
1. Land `Document` + tests with the existing single-TextEdit editor still wired to `Document::whole_text()`. Editor unchanged externally; storage swapped underneath.
2. Migrate decoration anchors to paragraph-relative.
3. Swap the editor render to per-paragraph widgets.
4. Wire undo on top of the now-stable `Document`.

Each step keeps the app working end-to-end; the flicker fix arrives at step 3.

### What we're betting on
- That the bridge cost (materialise paragraph → edit → write back) is small relative to the global re-layout we're eliminating. Profile after step 3.
- That cross-paragraph cursor handling can be implemented at a controller layer over multiple TextEdits without writing a custom widget. Helix and lapce both do this; egui has the primitives (focus, `TextEditState::store`, `CCursor`).
- That the writer experience is genuinely indistinguishable from today's single-TextEdit for the basic typing path. If subtle differences emerge (e.g. cursor blink seam at paragraph boundaries), they are addressed inside this ticket, not deferred.
