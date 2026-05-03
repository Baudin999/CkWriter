# 0013 — FEA: continuity_check skill + inspector surface (first user-visible consumer of the stack)

**Type:** FEATURE
**Created:** 2026-05-03
**Depends on:** #0008, #0009, #0010, #0011
**Optional:** #0012 (semantic retrieval makes "where else is this discussed?" cheaper, but the v1 skill works without it)

## Problem
Tickets #0008–#0012 build a substrate (storage layers, skill framework, transcoder, always-loaded context, optional semantic retrieval). Without a concrete user-visible feature pulling on that substrate, the bundle risks being **infrastructure-for-its-own-sake** — exactly the failure mode flagged in `docs/memory-architecture.md` §9 ("shipping discipline") and during ticket review.

This ticket ships the first end-to-end coaching feature on the stack: a **continuity_check** skill that uses typed accessors + chapter rollups + scene records to find contradictions between Author Layer canon and Manuscript prose.

Concrete examples it should catch:
- Author Layer says `characters.get("aren").eyes = "blue"`. Manuscript paragraph in ch.7 says "his grey eyes narrowed." → Flag.
- A `ThreadRef { thread_id: "silver-coin", label: "cursed coin" }` appears in `threads_opened` of ch.3 scene N, with no `threads_closed` entry anywhere downstream → "Open thread."
- `timeline.get("event_storm").when = "1247-Spring"`, but a `SceneRecord.beats` for the storm scene anchors to a chapter the timeline orders later → Flag.

If this skill produces useful flags, the whole stack pays off. If it doesn't, we re-design — better to learn that on a thin shipping surface than after building five infra tickets.

## Scope

### Skill definition
Registered in `src/agent/skills.rs`:
```rust
pub const CONTINUITY_CHECK: Skill = Skill {
    name: "continuity_check",
    version: 1,
    description: "Find contradictions between Author Layer canon and the Manuscript / SceneRecords.",
    model: ModelChoice::Default,
    tools: &[
        "characters.get", "characters.list", "characters.mentions",
        "locations.get", "locations.list",
        "timeline.get", "timeline.range", "timeline.around",
        "agent.recall",                  // pulls SceneRecords + ChapterRollups
        "manuscript.read",               // for quoting offending prose
    ],
    inputs:  SkillInputSchema::ContinuityScope,   // {scope: WholeBook | Chapter(name) | ChapterRange}
    outputs: SkillOutputSchema::ContinuityFlags,
    system_prompt: include_template!("continuity_check.md"),
    max_tool_calls: 32,
};
```

### Output type
Lives in `src/book/agent_layer.rs`:
```rust
pub struct ContinuityFlag {
    pub meta: AgentArtifactMeta,
    pub kind: ContinuityKind,            // CanonContradiction | OpenThread | TimelineConflict | UnboundEntity
    pub severity: Severity,              // Low | Medium | High
    pub summary: String,                 // one-line description
    pub explanation: String,             // 2–4 sentences
    pub anchors: Vec<FlagAnchor>,        // {paragraph_id, scene_id?, entity_id?}
    pub suggested_resolutions: Vec<String>, // free-form prose, ranked
    pub confidence: Confidence,
}

pub enum Severity { Low, Medium, High }
```
- Persisted at `Info/agent/continuity_flags.json` (single file; flag count is bounded).
- Each flag carries a stable `id` (content hash of `kind + anchors + summary`) so re-running the skill doesn't double-record an unchanged flag.

### Trust boundary
- The skill **does not** call `author.propose`. Its job is to surface contradictions, not to resolve them. The author resolves a flag by either:
  - Editing the Manuscript (the flag goes stale and is filtered out next render).
  - Editing the Author Layer (same: flag goes stale).
  - Dismissing the flag from the inspector (recorded as a `dismissed_flag_id` in the same file; dismissed flags suppress re-emission unless the underlying anchors change).

### Inspector surface
A new tab in the right panel: **Continuity** (alongside Chapter, etc.).
- Lists current flags grouped by chapter, sorted by severity descending.
- Each flag row: severity badge, summary, expand-to-show explanation + anchors + suggestions.
- Anchors are clickable: clicking jumps the editor to the paragraph (uses `current_paragraphs[].char_range` from #0002).
- "Run continuity check" button at the top: scope picker (whole book / current chapter / chapter range), runs the skill, refreshes the list.
- "Dismiss" action per flag.
- Stale flags (an anchor's paragraph hash no longer matches `meta.source_hash`) are auto-hidden; a small "N stale flags hidden" line is shown so the user knows.

### Validation rules
The orchestrator already validates `SkillOutputSchema::ContinuityFlags` shape. Skill-specific checks before persistence:
- Every `paragraph_id` in `anchors` exists in the current paragraph index. References to nonexistent ids → reject the whole flag (logged, dropped — not a hard skill failure, since the model may emit some bad anchors among many good ones).
- Every `entity_id` in `anchors` resolves via the typed accessors. Same handling.
- A flag with empty `anchors` → reject (a flag with no anchor is unactionable).

### Logging
Per the orchestrator's invocation log, plus skill-specific:
- Flags emitted (total + per kind + per severity).
- Flags rejected by validation, with reasons.
- Pre-existing flags suppressed (id collision with prior run).

## Out of scope
- Voice-drift, pacing-audit, foreshadowing-tracker — separate skills, separate tickets.
- Auto-fix actions (e.g. "click to update Author Layer to match Manuscript"). Doing this would require `author.propose` and a dedicated UI, neither of which is necessary for the v1 surface.
- Continuous background runs of the skill on save. v1 is button-triggered.
- A "continuity score" displayed somewhere prominent — premature; let the flag list stand on its own first.
- Cross-book continuity (would require a multi-book Author Layer; deeply out of scope).

## Acceptance criteria
- [ ] `CONTINUITY_CHECK` skill is registered; tools all resolve.
- [ ] Running the skill against a fixture book with a planted canon contradiction (character eye colour mismatch) emits a `CanonContradiction` flag with both anchors (the entity and the offending paragraph).
- [ ] Running the skill against a fixture with a `threads_opened` entry but no matching `threads_closed` emits an `OpenThread` flag anchored at the opening paragraph.
- [ ] A flag with a nonexistent `paragraph_id` is dropped with a logged reason; the rest of the flags persist.
- [ ] A flag with empty `anchors` is dropped.
- [ ] Re-running the skill without source changes does not double-record flags (same content hash → same id → upsert).
- [ ] Editing the Manuscript so an anchor's paragraph hash changes makes the flag stale; the inspector hides it (asserts `.visible_flag_count` decreases by 1).
- [ ] Dismissed flags do not re-emerge on a re-run unless one of their anchors changes.
- [ ] Inspector tab renders flags grouped by chapter, sorted by severity desc.
- [ ] Clicking an anchor in the inspector scrolls the editor to that paragraph.
- [ ] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- **Why this is the bundle's "shipping target."** The other tickets are infrastructure. This is the user-visible feature that justifies all of them. If continuity_check produces bad flags, the architecture is wrong; if it produces useful flags, every future skill (voice_drift, pacing_audit, etc.) inherits the same substrate for free. Ship it as the final ticket of the bundle, not first — earlier-ticket designs are validated by working through this one's needs.
- **Why no `author.propose` in v1?** Auto-proposing changes on every flag would create proposal-queue spam. The flag itself is the surface; the author resolves by editing source. A future ticket can add per-flag "propose this fix" actions once the flow is real.
- **Why dismissal storage in the same file as flags?** Dismissals are bound to flag ids; co-locating keeps consistency simple. This mirrors the pattern of #0003's per-chapter suggestion store.
- **Why button-triggered, not on-save?** Continuity check is whole-book in scope; running it on every save is wasteful. Let the writer trigger it when they're at a natural review point. Adding an "auto-check on chapter close" toggle is a follow-up.
- **Why severity as 3 buckets?** Same argument as `confidence` in #0008 — a coarse, honest bucketing avoids false precision. Severity is the model's self-rating; the writer makes the final call.
- **Why does this ticket include UI work?** Because the point is to validate end-to-end. Shipping a skill with no surface that displays its output would leave the question "is this useful?" unanswered. The inspector tab is small (list + buttons) and is the test of the whole stack.
- **Token budget.** Always-loaded context (~21k from #0011) + scene records for the scoped chapters (≤50k for whole book) + the model's reasoning ≈ 80k peak. Comfortable in gemma4's 124k window.
- **What's the Author Layer "canon"?** Per `docs/memory-architecture.md` §2.2: typed entity fields (eyes, age, role, etc.), entity-intent annotations, and `TimelineEvent` ordering. The skill's prompt enumerates which fields to check; adding a new field becomes a one-line prompt edit.
