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
- [ ] Re-running show/prose/spelling with no edits issues 0 prompts (all cached)
- [ ] Editing one paragraph and re-running issues exactly 1 prompt
- [ ] Cache invalidated only for paragraphs whose hash changed
- [ ] Token usage per run is logged
- [ ] Voice pipeline retains chapter-level behavior
- [ ] Anchoring, suggestion panel, and dismissal/accept flows unchanged from the user's perspective
- [ ] `cargo clippy` and `cargo test` clean

## Design notes
- Voice prompt + roadmap + cast preamble still applies on each paragraph for show pipeline.
- Per-paragraph prompts will be much smaller than 32k context — drop `num_ctx` accordingly to save memory and reduce model warmup time.
- Cache is per-pipeline-per-paragraph because the same paragraph might be cached for show but not prose if the writer ran one and not the other.
- Decision deferred: should a paragraph hash change invalidate ALL pipelines' cache for that paragraph, or only the pipelines whose outputs are quote-anchored in that paragraph? Default v1: invalidate all (simpler, conservative).
