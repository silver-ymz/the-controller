# Changelog

## v0.2.0

### Features

- **Tmux-backed sessions** — sessions now run inside tmux for persistence across app restarts
- **Session archival** — archive/unarchive sessions with confirmation modals
- **Vim-style navigation** — `j`/`k` navigation, `l`/Enter to focus, fuzzy finder
- **Hotkey system** — Escape leader key, `HotkeyManager` state machine, status bar hints
- **Terminal focus management** — Esc→h for sidebar, Esc→l for terminal, double-Escape to unfocus
- **Sidebar redesign** — collapse/expand, create session hotkey, focus indicators
- **Focus-after-delete** — redirect focus after session or project deletion (#10)
- **Auto-focus terminal** — auto-focus terminal after 2s sidebar dwell
- **Session continuation** — use `claude --continue` for restored/unarchived sessions
- **Session kind field** — thread session kind through `create_session` and PTY spawning
- **Duplicate project rejection** — reject duplicate project names in create/load/scaffold
- **Keyboard shortcut help** — `HotkeyHelp` modal showing all keyboard shortcuts

### Bug Fixes

- Fix duplicate paste by dropping custom Cmd-V handler
- Block Shift+Enter on all event types to prevent double-send
- Pass `--continue` flag when unarchiving project sessions
- Route Shift-Enter through tmux `send-keys` to bypass outer terminal parser
- Clear dwell timer on single-Escape from session focus
- Fix Cmd-V paste popup and Shift-Enter tmux format mismatch
- Restore sessions on reboot instead of destroying them
- Use `project.archived` as source of truth for archive filtering
- Reset leader timeout on modifier keypress to allow shifted keys
- Guard modifier-only keys in leader mode

## v0.1.0

Initial release.
