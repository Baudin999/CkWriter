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
