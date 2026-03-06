# Demo Recording

## Recording Setup

- **Screen recording:** macOS built-in (Cmd+Shift+5) — select window or region
- **Keypress overlay:** [KeyCastr](https://github.com/keycastr/keycastr) (`brew install --cask keycastr`) — shows keypresses as a floating overlay, important since The Controller is hotkey-driven

## Editing

Need to try both and pick one:

- **ffmpeg** — CLI, fast, no re-encoding: `ffmpeg -i input.mp4 -ss 00:01:30 -to 00:03:45 -c copy output.mp4`
- **iMovie** — GUI, timeline-based, drag-and-drop trimming

## Demo Ideas

- **Meta-programming lightshow:** Ask a session to change the app's own background color in a cycle — the controller editing itself in real-time.
- **Parallel bug swarm:** Open multiple sessions, each assigned a different GitHub issue on independent worktrees. Watch them all work simultaneously, then merge each one.
- **Self-hosting speedrun:** Use The Controller to build a new feature for itself (e.g., a theme toggle). Assign the issue, watch the session implement it, run tests, create a PR.
- **Issue triage assembly line:** Load the GitHub task panel with open issues. Rapidly assign each to a different session using the fuzzy finder and hotkeys — pure keyboard-driven orchestration.
- **Test-fix-verify loop:** One session runs tests and identifies failures. Assign each failing test to a separate session. Each fixes and verifies independently.
- **Image-driven UI polish:** Drag-and-drop a screenshot of a UI bug into a session. The session sees it and fixes the styling issue — shows multimodal input.
