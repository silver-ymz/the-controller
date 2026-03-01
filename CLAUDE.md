# The Controller

Tauri v2 + Svelte 5 desktop app for orchestrating multiple Claude Code terminal sessions.

## Key Docs

- `docs/domain-knowledge.md` — Hard-won lessons (Tauri main thread blocking, CLAUDECODE env var). **Read this before modifying Tauri commands or spawning processes.**
- `docs/plans/` — Design and implementation plans.

## Tech Stack

- **Frontend:** Svelte 5 (runes: `$state`, `$derived`, `$props`, `$effect`), xterm.js
- **Backend:** Rust (Tauri v2), portable-pty, git2
- **Theme:** Catppuccin Mocha

## Dev Commands

- `npm run tauri dev` — Run the app in development mode
- `cd src-tauri && cargo test` — Run Rust tests
