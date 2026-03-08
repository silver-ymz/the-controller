# Changelog

## v0.4.0

### Features

- **Maintainer background agent** — periodic code health checks with live countdown timer, `b` hotkey, and `r` to run checks (#132, #144, #146, #151)
- **Issue triage panel** — swiping-style triage with priority buckets, j/k hotkeys, complexity step, triaged labels, and category support (#147, #155, #172, #174, #181)
- **Screenshot-to-session** — Shift+S shortcut for screenshot sessions, cropped screenshots with optional preview (#170, #153)
- **Keystroke visualizer** — Cmd+K to toggle on-screen keystroke display (#185)
- **Global skills injection** — inject skills globally via home dir symlinks on startup (#186)
- **Clickable terminal links** — WebLinksAddon integration for clickable URLs in terminal (#137)
- **Issue creation flow** — two-stage keyboard flow with high/low priority labels and colored dot indicators (#157, #145, #165)
- **Merge hotkey improvements** — confirmation popup, squash-only merge for new projects, finishing-a-development-branch skill (#136, #113, #116)
- **Auto-cleanup worktrees** — automatic worktree cleanup after finishing a development branch (#126)
- **Keyboard shortcuts help** — categorized into sections (#173)
- **Sort issues by priority** — issue picker sorted by priority with colored indicators (#165)

### Bug Fixes

- Fix deadlock in handle_cleanup due to reversed lock ordering (#179)
- Fix scroll terminal to bottom when navigating to a session (#180)
- Fix triage hotkeys firing when modifier keys are held (#183)
- Show individually archived sessions in archive view (#175)
- Restore focus and active session after backend-initiated cleanup (#167)
- Handle session cleanup directly in backend instead of frontend (#162)
- Fix screenshot paste race conditions (#125, #130, #164)
- Fix PTY scroll race by registering output listener before connect_session (#134, #148)
- Fix terminal garbling by matching local PTY size to tmux on reattach
- Unarchive project when loading by repo_path (#133)
- Fix session branch name collisions with UUID suffix (#128)
- Fix codex merge hotkey and paste handling (#123, #127, #129)
- Pass THE_CONTROLLER_SESSION_ID via tmux -e flag for reliable env propagation (#131)
- Fix worktree-compatible finishing-a-development-branch skill (#119)
- Fix garbled terminal output when xterm.js opens in hidden container
- Eliminate extra newlines in sessions after restart

### Refactoring

- Modularize tauri command domains (#110)
- Simplify frontend architecture and remove obsolete UI glue
- Move triage ranking controls to right panel (#184)
- Move maintainer status from sidebar dot to panel text label (#171)
- Remove dead code and consolidate duplicate type definitions (#117)

## v0.3.0

### Features

- **GitHub issue integration** — assign issues to sessions, issue picker modal, issue context injection, in-progress labels (#22, #43)
- **Issue creation** — create GitHub issues via `i` hotkey with modal and toast notifications (#21, #31)
- **Task panel** — GitHub task list panel with `t` hotkey and auto-refetch (#12)
- **Session status hooks** — hook-based status events replacing PTY-output debounce (#28)
- **Merge hotkey** — merge session branch via `m` key with rebase + PR flow (#9, #20)
- **Drag-and-drop images** — handle drag-and-drop and paste images in terminal (#34, #36)
- **Codex sessions** — `x` for codex with issue picker, `X` for raw codex session
- **Session summary pane** — display initial prompt and git commit progress
- **Skill discovery** — Codex skill discovery via symlinks, project-local superpowers skills
- **Project scaffolding** — scaffold with template agents.md, GitHub remote creation (#25, #38)
- **Worktree naming** — use project name in worktree paths with migration (#14)
- **Background workers** — C/X hotkeys for autonomous background worker sessions
- **Persist session commits** — DONE state survives merge/rebase (#93)
- **GitHub issue cache** — in-memory GitHub issue cache (#89)
- **Image paste support** — paste images in Claude sessions (#30)
- **Three-state status indicator** — session status indicator with three states (#8)
- **Auto-start on issue** — auto-start assistant work when session has GitHub issue
- **.env copying** — copy .env from repo into new session worktrees

### Bug Fixes

- Fix merge conflict prompt sent to Claude Code session (#29)
- Fix idle transition flickering between tool calls (#28)
- Use carriage return instead of newline for PTY merge command
- Scope drag-drop image paste to active session only
- Gate terminal input during initialization to prevent spurious newlines
- Force terminal repaint and PTY resize when navigating back to session
- Use --append-system-prompt for issue context
- Fix issue picker keyboard nav and hooks format compatibility
- Show sidebar when escaping from terminal to session

### Testing

- Add 45 new Rust unit tests across all modules (#73)

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
