# 0006 — FEA: Embedding-based fuzzy suggestion dedup

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0003

## Problem
String normalization in the dismissal filter catches whitespace and case variation but the model often produces paraphrased duplicates of dismissed flags ("redundant adverb 'really'" vs. "this 'really' is filler"). After #0004 ships, paragraph-level caching kills most cross-run duplication, but fuzzy duplicates within a single paragraph's run may still surface.

**Build only after measuring**: this ticket is here so the idea isn't lost. Decide whether it's worth doing after #0004 has been in use long enough to see how much fuzzy duplication actually remains.

## Scope
- Local embedding via Ollama using `nomic-embed-text` (or similar small embedding model)
- For each new `proposed` flag: embed `quote + why`; compare cosine similarity to embeddings of `dismissed` and `accepted` flags in the same paragraph
- Drop the new flag if `max_similarity > threshold` (start at 0.88, tune)
- Cache embeddings keyed by `suggestion_id` so we don't re-embed
- Settings toggle: "Embedding-based dedup" (default off until tuned)
- Per-run total embedding latency logged

## Out of scope
- Cross-paragraph fuzzy dedup
- Cross-chapter fuzzy dedup
- Anything cloud-hosted

## Acceptance criteria
- [ ] Manual test: dismiss flag with quote A; coach run produces a paraphrased flag with similar meaning; embedding dedup drops it
- [ ] Threshold is configurable in settings; default is conservative (high) — better to under-filter than over-filter
- [ ] Per-run embedding latency stays under ~2 s on a typical chapter
- [ ] Embedding cache survives close/reopen
- [ ] Toggle disabled by default; enabling it never crashes if Ollama lacks the embedding model (graceful warning instead)
- [ ] `cargo clippy` and `cargo test` clean

## Design notes
- Don't start work on this ticket until #0004 has shipped and we have data on residual duplicate rates. If duplicates are <5% of flags after #0004, this ticket is closed without implementing.
- Embedding cache file path: `Info/embeddings/<folder>/<name>.json` — sidecar per chapter, keyed by suggestion_id.
- Cosine similarity threshold tuning: gather a small labeled set of dup/non-dup pairs from real coach runs before locking the default.

## Status notes
Closed without building 2026-05-03. Per the ticket's own wait-and-see gate, the writer reports no observed paraphrase-duplicates surviving the two layers that already exist:

1. **Prompt layer** — `src/llm/prompts.rs::build_user_with_history` (#0025) sends prior Dismissed/Accepted quotes back to the model with explicit "do NOT include them in `flags`" wording on every per-paragraph run.
2. **Ingest layer** — `src/book/suggestions.rs::fuzzy_match_record_id` (#0025) drops new flags whose normalized quote substring-matches OR Jaccard-matches (≥ 0.7) any existing non-Stale record in the same `(pipeline, paragraph_id)`.

Embedding dedup remains a defensible third layer if paraphrase-dups ever break through both, but the cost (Ollama dependency, threshold tuning, latency budget) isn't justified by observed pain. Reopen if that changes.

The writer-supervised side of "this is the same flag as that one" — which embeddings can't do without curation anyway — is filed separately as #0027.
