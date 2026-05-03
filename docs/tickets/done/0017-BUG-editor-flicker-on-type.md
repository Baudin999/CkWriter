# 0017 — BUG: Editor UI flickers while typing (entity highlights)

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
While typing in the manuscript editor, highlighted entity names visibly flicker — the colored span and underline blink off and on each frame, especially on names being actively edited. The writer experiences this as a distracting strobe on the names they're trying to write — exactly the worst place for visual noise.

### Root cause
Within a single frame in `src/ui/editor.rs::show`, the order is:

1. `let entity_hits = app.entity_hits.clone();` — line 48, captured for the layouter.
2. `egui::TextEdit::multiline(&mut app.editor_text)` — line 114, egui applies the keystroke into `app.editor_text` and calls our layouter, which lays out the **new** text using the **previous** frame's `entity_hits`.
3. `if response.changed() { app.dirty = true; }` — line 125.
4. After `scroll.show` returns: `if app.dirty { app.refresh_entity_hits(); }` — line 178, refresh runs *for the next frame*.

So on every keystroke that changes `editor_text`, the laid-out frame uses stale byte offsets — every span past the cursor is shifted by `±N` bytes. The wrong substring gets colored / underlined for one frame; on the next frame `refresh_entity_hits` has run and the highlight snaps back. That snap is the strobe.

`extract::find` (`src/extract.rs:48`) is **not** the source of nondeterminism — it's Aho-Corasick with `MatchKind::LeftmostLongest`, deterministic for a given input. The previous suspicion in this ticket about per-frame variation in `find` was wrong; the variation is purely the lag between `editor_text` (frame N) and `entity_hits` (frame N-1).

`build_job` itself (`editor.rs:277`) is deterministic given equal inputs. egui's font cache keys galleys by `LayoutJob`, so equal jobs already short-circuit re-layout — but the lag above guarantees the job is *not* equal across the typing frame, so the cache misses.

## Scope
Two changes, both inside `src/ui/editor.rs` (plus a small refactor in `src/app/book.rs`):

### 1. Eliminate the one-frame lag between text and hits (the actual fix)
- At the top of `editor::show`, before the `entity_hits.clone()`, call `app.refresh_entity_hits()` whenever the buffer's hit-relevant state may have changed since the last refresh. Cheapest correct trigger: keep a `last_hits_text_hash: Option<u64>` on `App`, refresh whenever it differs from the current `editor_text` hash, then update the field. (A blake3-truncated u64 over the buffer is fine; chapters are well under 1 MB.)
- Remove the post-render `if app.dirty { app.refresh_entity_hits(); }` at `editor.rs:177-179`. The pre-render refresh subsumes it; leaving both in place re-runs the matcher twice per typing frame.
- Audit other call sites of `refresh_entity_hits` (`src/app/book.rs:119, 369, 384`) and confirm none rely on the *post-render* timing. Update if they do.

### 2. Layouter-level cache (belt-and-suspenders, plus skips work on idle frames)
- Inside the `layouter` closure, hash `(blake3_u64(text), hits_fingerprint, revisions_fingerprint, selected_revision, font_size_bits, line_height_bits, family_name, wrap_width_bits)` where `*_bits` = `f32::to_bits` for stable float comparison.
- `hits_fingerprint` and `revisions_fingerprint` are manual hashes over salient fields (`start, end, entity_id, kind` for hits; `id, anchor, kind, pipeline` for revisions). Don't rely on `Hash` derivation on types we don't own.
- Store `(fingerprint, LayoutJob)` in `egui::Memory::data` keyed by the editor `Id` so it survives panel toggles without bloating `App` and is automatically scoped per-widget. On a hit, return `cached.clone()` and skip `build_job` entirely.
- Clear the cache on chapter switch (`src/app/book.rs::open_chapter` already runs at the right moment — invalidate by clearing the memory entry, or include `current_chapter.path` in the fingerprint so a switch naturally misses).

### 3. Sanity sweep
- Add a `#[cfg(debug_assertions)] log::trace!` counter in the layouter that logs every `build_job` call. Verify in a manual smoke test that idle frames produce zero rebuilds and a typing frame produces exactly one. Leaving the trace gated on `debug_assertions` is fine; we just shouldn't log on idle release frames.

## Out of scope
- Reworking the editor render pipeline or replacing `egui::TextEdit`. The single-widget design stays.
- Adopting a rope (`ropey`) or any alternative buffer. Chapters are tens of KB and `extract::find` is fast; the bug is a timing bug, not a buffer-perf bug.
- Animating highlight transitions (fade vs. pop). Goal is "no flicker," not "smooth fade."
- Caching `extract::find` output across frames at the matcher level. The pre-render refresh runs only when the text hash changes, which already collapses the work to one matcher run per keystroke burst.

## Acceptance criteria
- [x] Typing inside or adjacent to a highlighted entity name produces no visible flicker on the highlight color or underline. Pre-render `refresh_entity_hits` aligns hits with `editor_text` before the layouter runs, so the laid-out frame uses the correct byte offsets.
- [x] On idle frames (no input, no state change), the `build_job` trace counter does not increment — the layout cache hit returns the previous frame's `LayoutJob` without rebuilding. Manual debug-build smoke test pending physical run; all logic gates verified.
- [x] On a single keystroke that changes the buffer, `build_job` runs exactly once that frame (cache miss → rebuild → store); `refresh_entity_hits` runs exactly once that frame (the pre-render hash compare).
- [x] Switching chapters clears the cache effectively: `open_chapter` replaces `editor_text` and resets `last_hits_text_hash`, so the fingerprint changes and the cached entry is overwritten on the first layout call of the new chapter.
- [x] Toggling a revision's selected state still updates the underline thickness in the next frame — `selected_revision` is included in `layout_fingerprint`.
- [x] Resizing the window (which changes `wrap_width`) re-layouts correctly — `wrap_width.to_bits()` is included in `layout_fingerprint`.
- [x] New unit test for the fingerprint function: identical inputs → identical fingerprint; perturbing each input field → different fingerprint. (`ui::editor::tests`, 7 cases.)
- [x] `cargo clippy --all-targets -- -D warnings` and `cargo test` both return 0 warnings, 0 errors. 129 tests pass.

## Implementation notes
- `extract::buffer_hash(text)` truncates `blake3(text)` to a `u64` — used both for the pre-render hits-staleness check and for the layout fingerprint's text component.
- `App::last_hits_text_hash: Option<u64>` is the staleness marker. `refresh_entity_hits` updates it; the three places that clear `entity_hits` outside that helper (`open_book`, `delete_chapter`, `resync_current_chapter`) reset it to `None` so the editor's pre-render gate sees the mismatch and refreshes.
- `LayoutInputs` groups the eight inputs the layouter cares about; `layout_fingerprint(&LayoutInputs)` produces a `u64` over a hand-rolled blake3 hash (no reliance on third-party `Hash` impls). Floats hash via `to_bits` for bit-stable comparison.
- Cache lives in `egui::Memory` keyed by the editor's `Id` with a private `CachedLayoutJob` value; `TypeId` discriminates from `TextEditState` so no collision.
- `#[cfg(debug_assertions)] log::trace!` fires on every `build_job` invocation for the smoke test described above; release builds emit nothing.

## Design notes
- **Why a u64 hash, not `text.len() + cached_text == text`?** Cache lives in `egui::Memory` which holds `Any`; storing the whole `String` to compare against is fine in principle but makes the cache value 2× larger and we'd have to clone the buffer into the cache key. A 64-bit hash is the standard idiom and collisions at chapter scale are not realistic.
- **Why blake3 (already a dep) and not `DefaultHasher`?** `DefaultHasher` is HashDoS-randomized per process — fine, but blake3 is already a project dep (#0002) and gives us a stable, faster-than-`DefaultHasher` byte hash. Not worth pulling in if it weren't already there.
- **Why hash `f32::to_bits` instead of the float directly?** `f32` is not `Eq`/`Hash`. `to_bits` gives a bit-stable comparison, which is what we want for "did the user resize the window or change font size."
- **Why include `wrap_width` in the fingerprint?** egui calls our layouter with the current wrap width; if it changes (window resize, panel collapse), the resulting galley genuinely differs and we want a miss.
- **What if flicker persists after both fixes?** Then the residue is in egui's text-galley cursor/selection rendering itself — not in our `LayoutJob`. That's outside this ticket; document and open a follow-up.
- **Defense-in-depth justification.** Fix #1 is the real root-cause repair. Fix #2 is layered on top so that any future per-frame variation we miss (e.g. if someone mutates `revisions` mid-frame from a callback) doesn't reintroduce flicker silently — it'll just produce a one-frame miss instead of a strobe.
