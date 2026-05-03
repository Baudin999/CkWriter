# CkWriter — project conventions

## Ticket-driven workflow

All feature, refactor, and bug work in this project flows through tickets in `docs/tickets/`.

### Directory layout
- `docs/tickets/backlog/` — ideas and not-yet-prioritized work
- `docs/tickets/todo/` — refined and ready to start
- `docs/tickets/inprogress/` — currently being worked on
- `docs/tickets/done/` — completed
- `docs/tickets/number.txt` — next ticket number, monotonically increasing
- `docs/tickets/TEMPLATE.md` — copy this when filing a new ticket

### Status is the directory
A ticket's status is its current directory. **Never duplicate status in the frontmatter** — that creates two sources of truth that drift. To change a ticket's status, move the file.

### WIP limit
At most **1 ticket** in `docs/tickets/inprogress/` at a time. If work is interrupted, move the in-progress ticket back to `todo/` (not `backlog/`) with a `## Status notes` section appended summarizing what's done and what remains. `backlog/` is for "not started"; `todo/` is for "refined and ready, possibly mid-flight."

### Filename format
`NNNN-ABB-slug.md` — e.g. `0001-FEA-chapter-metadata.md`.

Type abbreviations:
- `FEA` — FEATURE (new capability)
- `REF` — REFACTOR (restructure; small behavior changes are OK only in service of structure)
- `BUG` — BUG (fix to wrong behavior)
- `DOC` — DOCUMENTATION (writing/editing project docs)

### Numbering
Read `docs/tickets/number.txt`. The integer in that file is the **next** ticket number. After creating a ticket, write the incremented value back. Numbers are never reused, even if a ticket is deleted.

### When NOT to file a ticket
Tickets are for substantive work. Skip the ticket for:
- Typo fixes
- Build errors / clippy warnings on existing code
- Exploratory questions ("how does X work?", "what would Y look like?")
- Follow-ups discovered while working an in-progress ticket — append to that ticket's notes instead
- One-line config tweaks

When in doubt, ask the user.

### Working a ticket
1. At session start, read `docs/tickets/inprogress/`. If something is there, ask the user whether to continue it or pick a different one. If empty, ask which ticket from `todo/` to start.
2. While working, cite the ticket id in commit messages (e.g. `feat(#0001): ...`).
3. If interrupted, append a `## Status notes` section and move the file to `todo/`.
4. On completion, check off acceptance criteria, move the file to `done/`, and commit.
5. Never start a second `inprogress` ticket without first parking the existing one back in `todo/`.

## Quality gates
This project follows the global quality non-negotiables (see `~/.claude/CLAUDE.md`). Before reporting any ticket complete: `cargo clippy` and `cargo test` must both return zero errors and zero warnings. If a ticket can't reach zero, say so explicitly and either lift the gate (with justification in the ticket) or extend the ticket — never silently lower severity.
