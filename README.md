# CkWriter

A local, LaTeX-aware book-writing companion built in Rust + egui, with character/location hover info from a per-book JSON database and three coaching pipelines (voice, show-don't-tell, prose) backed by a local Ollama model.

Built as a sister project to [CkEditor](../CkEditor); designed specifically for Carlos Kelkboom's *The Redemption Chronicles*.

## What it does

- Opens a LaTeX book directory (one with `main.tex` using `\include{...}` for chapters).
- Shows the chapter list (in `\include{}` order plus loose `.tex` files greyed out).
- Edits chapter `.tex` files in place. `Ctrl+S` saves.
- Recognises character and location names from `Info/Characters/*.json` and `Info/Locations/*.json`, underlines them in the prose, and shows a tooltip card on hover.
- Right panel: in-scope characters and locations for the current chapter (sorted by frequency), with an inline inspector to edit name / role / age / tone / voice notes / aliases.
- Bottom AI panel: three buttons (`voice`, `show, don't tell`, `prose`) stream Gemma's structured-JSON suggestions, anchor each one to a span of prose, render colored squiggles in the editor, and offer Accept / Dismiss in a sidebar.
- The system prompt sent to the LLM is built from your own `Info/Writing Assistant/voice-system-prompt.md` plus the tail of `Info/World Building/Plot.txt` (roadmap), plus per-character voice notes for everyone in the current scene.

## Database layout (per-book)

```
<book>/
  main.tex
  Ancient/, Modern/                       chapter .tex files
  Info/
    index.json                            optional book config (model, prompt paths)
    Characters/<slug>.json
    Locations/<slug>.json
    Events/<slug>.json
    Timeline/<slug>.json
    Writing Assistant/
      voice-system-prompt.md              loaded into LLM system prompt
    World Building/
      Plot.txt                            tail loaded as roadmap
```

Per-entity JSON schema (all fields optional except `id` and `name`):

```json
{
  "id": "amenophis",
  "name": "Amenophis",
  "aliases": ["Deina", "Lady Amenophis"],
  "role": "Higher Being, warrior",
  "age": "ancient",
  "tone": "commanding, reflective",
  "voice_notes": "speaks rarely; when she does, it lands.",
  "relations": [{ "kind": "ward", "id": "nefar" }],
  "first_seen": "Ancient/001_Wua",
  "tags": ["higher-being", "ancient-arc"],
  "free_text": "..."
}
```

The first time you open a book that has a `Info/Characters/Personae.txt`, the right panel offers a one-click *Import from Personae.txt* button to seed the database. Idempotent — never overwrites existing JSON.

## Running

```bash
cargo run --release
```

A welcome dialog asks for the book root. Defaults to `~/Projects/TheRedemptionChronicles`. Recent books persist in `~/.config/ckwriter/settings.toml`.

## Configuration

`~/.config/ckwriter/settings.toml` (auto-created):

```toml
model = "gemma4:latest"
ollama_url = "http://localhost:11434"
editor_font_size = 16.0
recent_books = []
```

Per-book override at `<book>/Info/index.json`:

```json
{ "model": "gemma4:latest" }
```

## Keyboard

- `Ctrl+S` — save current chapter
- `Ctrl+Shift+S` — save chapter notes scratchpad

## Status

v1. Light on quality gates by design (`cargo fmt` and default `cargo clippy` only).

## Not yet

- Candidate proper-noun finder ("?" tab for unknown capitalised words)
- File watcher for external (Overleaf) edits
- Settings UI panel
- Vim mode / syntect highlighting (using plain `egui::TextEdit` for v1)
- Multi-book tabs
- Persisted revision history
- PDF/EPUB export — keep using `latexmk` / Overleaf for that
