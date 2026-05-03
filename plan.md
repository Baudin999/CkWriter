# Plan: unified streaming-JSON pipeline for AI panels

## Why

Three panels (coach revisions, character extraction, character progression) all
do the same five steps: build prompt → open Ollama stream → poll → parse JSON →
hand the typed result to a callback. Today only the coach panel has the
hardened parse path (strict → salvage-array → dump-to-disk → ask the model to
repair). Characters and progression have an ablated copy with strict parse
only, so when gemma emits the same malformed-array shape coach handles fine,
those panels just give up. Today's failure: `characters` parse died at
`line 47 col 29` on a stray `undue_weight":` token — the exact pattern
`salvage_array` already has unit tests for.

## Current drift

| Concern | coach | characters | progression |
|---|---|---|---|
| Stream slot | `stream` | `char_stream` | `progression_stream` |
| Pipeline tag | `stream_pipeline` | — | `progression_target` |
| Repair flag | `stream_is_repair` | — | — |
| Buffer keepsake | `last_stream_buffer` | `last_char_buffer` | — |
| Status field | `last_error` | `char_extract_error` | `progression_status` |
| Strict parse | yes | yes | yes |
| Salvage-array fallback | yes (`flags`) | **no** | n/a (object-shaped) |
| Dump unsalvageable to `test-results/` | yes | **no** | **no** |
| LLM repair retry | yes | **no** | **no** |
| Tuning ctx / predict | 32k / 2k | 32k / 4k | 32k / 1k |

The drift is incidental, not designed: coach got hardened over the last few
sessions, the other two were left at v1.

## Target shape

One generic job type owns everything that isn't pipeline-specific.

```rust
// src/llm/job.rs (new)

pub struct LlmJob<T> {
    handle:           StreamHandle,
    salvage:          SalvageStrategy,
    label:            &'static str,        // for logging + dump filename
    repair_schema:    &'static str,        // example JSON shape, used in repair prompt
    is_repair:        bool,
    original_failure: Option<String>,      // kept across the repair retry so the
                                           // post-repair dump records the true source
    settings:         JobSettings,         // ollama_url, model, ChatTuning for repair
    _marker:          PhantomData<fn() -> T>,
}

pub enum SalvageStrategy {
    /// `{ "<key>": [ ... ] }` — recover individual array elements.
    Array { key: &'static str },
    /// Single object — no per-element salvage; repair-retry is the only fallback.
    Object,
}

pub enum JobOutcome<T> {
    Pending,
    Done(T),
    Failed(String),     // already logged + dumped; UI just shows this
}

impl<T: DeserializeOwned> LlmJob<T> {
    pub fn poll(&mut self) -> JobOutcome<T> { ... }
}
```

Each panel owns one `Option<LlmJob<PanelT>>` slot. App-tick pseudocode:

```rust
if let Some(job) = self.coach_job.as_mut() {
    match job.poll() {
        JobOutcome::Pending      => {}
        JobOutcome::Done(result) => { self.coach_job = None; self.ingest_coach(result); }
        JobOutcome::Failed(msg)  => { self.coach_job = None; self.last_error = Some(msg); }
    }
}
```

The repair retry happens **inside** `LlmJob::poll`: when strict + salvage both
fail and `is_repair == false`, the job replaces its own `handle` with a new
repair stream and flips `is_repair = true`. Outside callers never see the
repair as a separate state. `original_failure` is stashed so that if the
repair attempt also fails, the dump records the original broken response, not
the repair-of-the-broken-response.

What collapses out of `CkWriterApp`:

- `stream`, `char_stream`, `progression_stream` → three typed `LlmJob<_>` slots
- `stream_pipeline`, `stream_is_repair`, `progression_target` → carried inside the job
- `last_stream_buffer`, `last_char_buffer` → carried inside the job until terminal state, then dropped (or persisted via `Failed(msg)` with the buffer in the dump file)
- Three different status fields → still three, but each panel's status is now derived from `JobOutcome`, not bespoke flags

## Migration order

Each step compiles, passes tests, and is independently committable.

1. **Move salvage + dump + repair out of `coach.rs`.** New file
   `src/llm/job.rs`. Lift `dump_unsalvageable` and the repair-prompt
   construction verbatim. Keep coach using its current state machine for now;
   coach just calls into the new helpers. Tests: existing coach tests still
   pass.

2. **Define `LlmJob<T>` and `SalvageStrategy`.** Add unit tests for the four
   transitions:
   - strict succeeds → `Done`
   - strict fails, salvage recovers ≥1 element → `Done`
   - strict + salvage both fail, repair issued, repair succeeds → `Done`
   - repair fails → `Failed`, dump file present on disk
   - unsalvageable + `Object` strategy → repair issued (no salvage attempt)

3. **Migrate coach to `LlmJob<CoachResult>`.** Where `CoachResult` carries the
   parsed flags plus the pipeline tag (so `ingest_coach` knows whether to
   colour by spelling-kind or by pipeline). Delete `stream`,
   `stream_pipeline`, `stream_is_repair`, `last_stream_buffer`. Verify same
   behaviour against the existing dumped fixtures in `test-results/`.

4. **Migrate character extraction.** `LlmJob<RawCharacters>` with
   `SalvageStrategy::Array { key: "characters" }`. Delete `char_stream`,
   `last_char_buffer`. Add fixture test: today's failing payload (saved
   below) parses via salvage to ≥3 characters. Verify the JSON-repair retry
   actually fires when salvage returns zero.

5. **Migrate progression.** `LlmJob<RawProgression>` with
   `SalvageStrategy::Object`. Delete `progression_stream`. Keep
   `progression_target` since it's chapter context, not stream state.

6. **Forbid the old shape.** Add an architectural lint: `parse_json_object`
   may only be called from `src/llm/job.rs` and tests. Anything else is a
   panel reaching past the unified pipeline. Error-level, no allow-list.

## Per-panel spec (only this should differ between panels)

Once unified, each panel contributes exactly:

- **System prompt** (constant string)
- **User prompt builder** `(panel inputs) -> String`
- **Output type** `T: DeserializeOwned`
- **Salvage strategy** (`Array { key }` or `Object`)
- **Repair schema example** — the one-line JSON shape included in the
  repair-system-prompt so the model knows the target. (Currently
  hardcoded to the flags shape inside `start_json_repair`; needs to be
  parameterised.)
- **Tuning** (`temperature`, `num_ctx`, `num_predict`)
- **Success callback** `fn(&mut CkWriterApp, T)`

Worked example:

| Field | coach (spelling) | characters | progression |
|---|---|---|---|
| `T` | `RawFlagsOnly` | `RawCharacters` | `RawProgression` |
| salvage | `Array{"flags"}` | `Array{"characters"}` | `Object` |
| schema | `{"flags":[{"kind":...,"quote":...,"why":...,"suggestion":...}]}` | `{"characters":[{"name":...,"aliases":[...],"role":...,"voice_notes":...,"evidence":...}]}` | `{"tone":...,"situation":...,"voice_summary":...,"notable_changes":...}` |
| predict | 2k | 4k | 1k |

## Quality gates (per CLAUDE.md)

- All new transitions covered by `#[test]` in `src/llm/job.rs`.
- Existing coach fixtures from `test-results/` wired into the test suite as
  regression cases (move them into `src/llm/parse.rs::tests` as
  `include_str!`).
- The "no `parse_json_object` outside `llm/job.rs`" rule lands as an
  error-level architectural lint in the same PR — not as a follow-up.
- Zero new warnings on `cargo build` and `cargo clippy --all-targets`.

## Data point — today's failure

Saved log line (from `~/.local/state/ckwriter/ckwriter.log`,
`2026-05-02T19:52:50Z`): characters parse failed at line 47 col 29; preview
shows the array breaking on:

```text
    {
      "name": "Lord Azariel",
      "aliases": ["Az"],
      ...
    },
    undue_weight": "..."     <-- stray key with no enclosing object
```

Identical structurally to `salvage_recovers_when_stray_fragment_has_no_opening_brace`
in `src/llm/parse.rs::tests`. Capture the full 2370-byte response from the
next reproduction into `test-results/` and use it as the regression fixture
in step 4.

## Out of scope

- Changing the prompts themselves. The premise is that prompts are the only
  thing that should vary between panels; the cleanup makes that true in code.
- Changing what each panel does with a parsed result. Anchoring, DB diff,
  and progression-append all stay where they are — they just get fed by the
  unified job instead of inline parse calls.
- Concurrent multi-panel runs. Today panels are mutually exclusive in
  practice; the per-panel `Option<LlmJob<_>>` slots are enough.
