# 0018 — BUG: Chat panel does not render markdown

**Type:** BUG
**Created:** 2026-05-03
**Depends on:** none

## Problem
The chat panel renders assistant and user messages with a plain `ui.label(RichText::new(content))` in `chat_bubble` (`src/ui/scope_panel.rs:1209-1224`). The model commonly returns markdown — bullet lists, **bold**, *italic*, `code`, fenced code blocks, headings — and the writer sees the raw markdown source instead of formatted text. Long replies with bullet lists are the most painful: they collapse into one wall of asterisks.

This isn't a model-prompting problem (asking the model to "write plain text" loses the structure that makes its replies useful). It's a render problem on our side.

## Scope
- Replace the plain `ui.label` for chat message bodies with a markdown renderer.
- Required markdown features (in order of importance):
  - Paragraph breaks (blank line → new paragraph).
  - Bullet lists (`- ` and `* `).
  - Numbered lists (`1. `, `2. `, …).
  - Inline `code` and fenced ```` ``` ```` code blocks (monospace, subtle background).
  - **Bold** and *italic*.
  - Headings (`#`, `##`, `###`) — small visual weight only; chat isn't a doc.
- Apply only inside `chat_bubble`. The role label (`you` / `ai`) keeps its current `RichText` styling.
- Streaming pending-assistant text (`app.chat_pending_assistant`, rendered at line 1172) must use the same renderer so partial markdown formats as it streams in.

## Out of scope
- Markdown rendering anywhere other than the chat bubbles (inspector, chapter-tab notes, etc.).
- Tables, blockquotes, footnotes, links with click handlers, images. If they show up, render as the underlying text — don't crash.
- Syntax highlighting inside fenced code blocks.
- A user-facing toggle "render markdown / show raw."

## Acceptance criteria
- [x] Sending a chat reply that contains a bullet list renders as a visual list, not raw `- ` characters.
- [x] **Bold** and *italic* render as bold and italic, not with the asterisks visible.
- [x] Fenced code blocks render in a monospace font with a distinguishing background; inline `code` renders in monospace inline.
- [x] Streaming partial markdown renders incrementally and doesn't visibly re-flow the entire bubble on every token (best-effort — re-flow is acceptable, full repaint stutter is not).
- [x] User messages also support markdown (the writer may paste it; consistent rendering is simpler than role-conditional rendering).
- [x] `cargo clippy` and `cargo test` clean (0 warnings, 0 errors).

## Design notes
- `egui_commonmark` (a maintained crate) is the obvious candidate. Add it to `Cargo.toml` and feature-gate if it pulls in unwanted deps. Verify its license is compatible.
- If `egui_commonmark` won't work, a hand-rolled minimal renderer (paragraph + bullets + bold/italic + code) is acceptable for v1; missing features render as their underlying text.
- Cache the parsed/rendered representation per message so the chat scroll area doesn't re-parse every frame. The pending assistant message is the only one that needs to re-parse on each token.
- Picking a renderer is itself a small design decision worth recording in this ticket's notes when the choice is made.

## Decisions

- **2026-05-04 — Renderer:** `egui_commonmark = "0.20"` with `default-features = false` and `features = ["pulldown_cmark"]`. v0.20 is the line that targets egui 0.31; license MIT OR Apache-2.0 (compatible). Default-off drops `load-images` (we don't render images in chat) and avoids pulling in `egui_extras` image stack. The crate already exposes a `CommonMarkCache` for reusing parsed/laid-out output across frames, which satisfies the per-message caching requirement without us hand-rolling one.
