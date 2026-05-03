# 0004 — FEA: Per-paragraph coach caching

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0002, #0003

## Problem
Even with dismissal filtering, every coach run today re-prompts the model with the entire chapter prose, costing 32k context every time and re-flagging unchanged paragraphs. A typical writer edits one or two paragraphs between runs — paying for the whole chapter every time is wasteful, slow, and the source of most "the model gave me different flags this time" noise.

This is the single biggest quality + cost win of the AI roadmap.

## Scope
- Coach pipelines (show, prose, spelling) run paragraph-by-paragraph instead of whole-chapter
- For each paragraph, check `last_run_hashes[pipeline][paragraph_id]` against current paragraph hash:
  - Match: skip — return cached `proposed` suggestions for that paragraph
  - Miss: prompt the model with that paragraph's prose only; ingest results; update cache hash
- Cache lives in `chapter.json` as `last_run_hashes: { pipeline_label: { paragraph_id: hash } }`
- Existing pipeline buttons still trigger a full pipeline run, but the run is now "for each dirty paragraph"
- Streaming UI shows `running paragraph K of N`
- Voice pipeline keeps its current chapter-level behavior (the score is chapter-level; gate on pipeline kind in the run dispatcher)
- Token usage logged per run

## Out of scope
- Splitting very long paragraphs into sentence chunks
- Cross-paragraph context (each paragraph prompted in isolation v1; revisit if quality drops)
- Chunked Voice scoring

## Acceptance criteria
- [x] Re-running show/prose/spelling with no edits issues 0 prompts (all cached)
- [x] Editing one paragraph and re-running issues exactly 1 prompt
- [x] Cache invalidated only for paragraphs whose hash changed
- [x] Token usage per run is logged
- [x] Voice pipeline retains chapter-level behavior
- [x] Anchoring, suggestion panel, and dismissal/accept flows unchanged from the user's perspective
- [x] `cargo clippy` and `cargo test` clean

## Design notes
- Voice prompt + roadmap + cast preamble still applies on each paragraph for show pipeline.
- Per-paragraph prompts will be much smaller than 32k context — drop `num_ctx` accordingly to save memory and reduce model warmup time.
- Cache is per-pipeline-per-paragraph because the same paragraph might be cached for show but not prose if the writer ran one and not the other.
- Decision deferred: should a paragraph hash change invalidate ALL pipelines' cache for that paragraph, or only the pipelines whose outputs are quote-anchored in that paragraph? Default v1: invalidate all (simpler, conservative).

## Implementation notes
- `ChapterMeta::last_run_hashes: BTreeMap<String, BTreeMap<String, String>>` keyed by `(pipeline_label, paragraph_id)`. Persists in `chapter.json`; legacy sidecars without the field load with an empty map.
- New `CoachRun` orchestration record on `App` carries the pending paragraph queue, current index, and aggregated token totals across one run. Only show/prose/spelling use it; voice keeps the chapter-level path in `start_voice_run`.
- `compute_dirty_paragraphs` is a pure function (testable) that returns `Vec<PendingParagraph>` snapshotted at queue-build time so a mid-run edit can't shift offsets under the loop.
- Streams chain inside `poll_stream`: a successful per-paragraph ingest writes `last_run_hashes[label][id] = hash`, advances the queue index, and either kicks off the next paragraph or finalizes. A malformed response still goes through the existing JSON-repair path; if even repair fails, the queue advances but the cache entry stays untouched, so next run retries that paragraph.
- `finalize_coach_run` prunes `last_run_hashes[label]` of paragraph_ids no longer present (paragraph deletes don't leave dead cache entries) and logs `prompt_tokens=… eval_tokens=… total_tokens=… aborted=…`.
- Per-paragraph runs use `num_ctx=8192` (system preamble + one paragraph + JSON output fits comfortably); voice keeps `num_ctx=32768`.
- `StreamHandle` gained `prompt_eval_tokens` / `eval_tokens` fields populated from a new `StreamEvent::Stats` event sent immediately before `Done`, so the coach run loop can sum totals without re-parsing log lines.

## Verification
- `cargo clippy --all-targets -- -D warnings` clean.
- `cargo test`: 139/139 pass. New tests cover (a) `last_run_hashes` round-trip on disk, (b) legacy sidecars without the field, (c) `compute_dirty_paragraphs` empty-on-match / single-on-edit / all-on-cold-cache.
- Not yet exercised live against Ollama — egui app wasn't run during this ticket. Smoke-test next session: open a chapter, hit prose twice (second run should log `0 prompts`), edit one paragraph, hit prose again (should log `dirty=1/N`), then run voice and confirm it still does the chapter-level path.
