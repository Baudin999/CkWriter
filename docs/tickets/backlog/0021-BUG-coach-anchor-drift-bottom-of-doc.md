# 0021 — BUG: Coach card "jump to span" drifts toward bottom of document

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
Clicking a spelling-coach (and other coach) card to jump the editor to the flagged passage works at the top of a chapter and breaks toward the bottom — the cursor lands in the wrong place, or the editor reports a span / boundary error. The drift is monotonic with depth into the document, which is the tell for a position-mapping bug.

Root cause is in `src/app/coach.rs:234-238`. The anchor is computed by trying three different *strings* but the result is stored as a single `(byte_start, byte_end)` that the rest of the system (`select_revision` → `jump_to_anchor` at `src/app/book.rs:460`) treats as byte offsets into `self.editor_text`:

```rust
let anchor_in_text = revision::anchor(&self.editor_text, &f.quote)
    .or_else(|| revision::anchor(&latex::to_prose(&self.editor_text), &f.quote))
    .or_else(|| revision::anchor(&self.editor_text, f.quote.trim()));
```

The middle fallback returns offsets into the **latex-stripped prose** — a different buffer. Every `\nl`, `\switch`, or `\emph{` above the quote is missing in that buffer, so the returned `byte_start` is short by the cumulative byte-length of every stripped token above the quote. Top of the document: zero stripped tokens above → no drift → works. Bottom: drift accumulates → cursor lands well before the quote, sometimes inside a multi-byte UTF-8 codepoint, which then panics or produces a wrong char-index in `editor_text[..cap].chars().count()` (book.rs:462).

The same pattern exists in the rehydration path (`coach.rs:447`), but that one anchors only against `editor_text` so it isn't affected. The bug is specifically the prose-fallback branch.

## Scope
- Stop returning prose-stripped offsets as if they index `editor_text`. Two acceptable fixes; pick one in design notes:
  1. **Drop the prose fallback.** Anchor only against `&self.editor_text` (with the existing whitespace-collapse and prefix-probe fallbacks already inside `revision::anchor`). Accept a lower hit rate for quotes the model paraphrased — those records still appear in the panel with `anchor = None` and sort to the bottom.
  2. **Map prose offsets back to raw offsets.** Extend `latex::to_prose` (or add a sibling) to return both the stripped string and a position map (analogous to `revision::collapse_map` at `src/llm/revision.rs:144,180`), then translate `(prose_start, prose_end)` back into raw `editor_text` offsets before storing on the record.
- Apply the same fix at the ingest path (`coach.rs:234-238`). Audit any other call sites that build an anchor by passing a *derived* string into `revision::anchor` and storing the result against the raw buffer; convert or drop them.
- Harden `jump_to_anchor` (`src/app/book.rs:460`) so that a `byte_start` landing on a non-char-boundary is detected and either snapped to the previous boundary with a warn-log, or ignored with a warn-log — never panics. This is defense-in-depth; the primary fix is upstream.
- Add a regression test in `src/app/coach.rs` (alongside `rehydration_re_anchors_raw_quote_in_live_text` at line 626): construct an `editor_text` containing several `\nl` / `\emph{...}` tokens, ingest a coach flag whose `quote` only matches after prose stripping, and assert that the resulting `anchor` indexes into `editor_text` such that `&editor_text[s..e]` returns the expected substring (or that `anchor` is `None` if option 1 is chosen).

## Out of scope
- Reworking how the model produces quotes. The fix is on our anchoring side; quote shape is unchanged.
- Improving the hit rate of `revision::anchor` beyond the current three-strategy stack — the prefix-probe at `revision.rs:150` stays as-is.
- Caching of layout / paragraph index. (#0017 covers flicker; #0002 already covers paragraph identity.)

## Acceptance criteria
- [ ] Clicking a coach card whose anchored passage is in the *last* paragraph of a multi-`\nl` / `\emph`-heavy chapter places the cursor at the correct byte and scrolls it into view.
- [ ] No `byte_start` outside `0..=editor_text.len()` and no `byte_start` on a non-char-boundary reaches `pending_cursor_char` — verified by either: (a) anchors are always raw-buffer offsets, or (b) `jump_to_anchor` rejects/snaps invalid inputs with a warn-log.
- [ ] New unit test covers the previously-broken case and fails on the current `master`.
- [ ] `cargo clippy --all-targets -- -D warnings` and `cargo test` both return zero warnings and zero errors.

## Design notes
Recommended path: **option 1 (drop the prose fallback)** unless we have evidence that it materially improves hit rate. The whitespace-collapse fallback already inside `revision::anchor` (`revision.rs:141-149`) handles the common case where the quote came back with normalized whitespace. The prose fallback was added to recover quotes the model returned in stripped form, but the cost is silent positional corruption — we'd rather show the record un-anchored (it sorts to the bottom of the panel) than place the cursor on the wrong sentence.

If telemetry/logs show the prose fallback was actually catching a meaningful number of flags, escalate to **option 2**: a `latex::to_prose_with_map(&str) -> (String, Vec<usize>)` returning a per-byte map prose→raw, plus an `anchor_with_map` that consumes it. Mirrors the existing `collapse_map` pattern.

The defense-in-depth in `jump_to_anchor` is cheap and worth keeping regardless of which option we pick — `&str[..n]` panicking on a non-boundary is a foot-gun we'd rather not relearn.
