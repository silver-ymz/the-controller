//! Service layer — shared business logic for Tauri commands and Axum handlers.
//!
//! Each public function in this module (and its submodules) encapsulates a
//! single unit of business logic. Both `commands.rs` (Tauri IPC) and
//! `server/` (Axum HTTP) delegate here, keeping the API surfaces thin.
//!
//! Errors are returned as [`crate::error::AppError`], which converts into
//! the appropriate response type for each API surface.

use std::path::Path;

mod auth;
mod config;
mod deploy;
mod github;
mod maintainer;
mod notes;
mod projects;
mod scaffold;
mod secure_env;
mod sessions;
mod voice;

pub use auth::*;
pub use config::*;
pub use deploy::*;
pub use github::*;
pub use maintainer::*;
pub use notes::*;
pub use projects::*;
pub use scaffold::*;
pub use secure_env::*;
pub use sessions::*;
pub use voice::*;

// ---------------------------------------------------------------------------
// Shared helpers used by multiple submodules
// ---------------------------------------------------------------------------

const DEFAULT_AGENTS_MD: &str = r#"# {name}

One-line project description.

## Task Structure (CRITICAL -- NEVER SKIP)

**This is the most important rule. Every task, no matter how small, MUST follow this structure before writing any code. No exceptions.**

1. **Definition**: What's the task? Why are we doing it? How will we approach it?
2. **Constraints**: What are the design constraints -- from the user prompt, codebase conventions, or what can be inferred?
3. **Validation**: How do I know for sure it was implemented as expected? Can I enforce it with flexible and non-brittle tests? I must validate before I consider a task complete. For semantic changes (bug fixes, feature refinements): if I revert my implementation, the test must still fail. After the implementation, the test must pass.

**If you catch yourself writing code without having stated all three above, STOP and state them first.**

## Key Docs

- `docs/plans/` -- Design and implementation plans.

## Tech Stack

<!-- Fill in your project's tech stack -->

## Dev Commands

<!-- Fill in your project's dev commands -->
"#;

/// Generate default `agents.md` content for a project.
pub fn render_agents_md(name: &str) -> String {
    DEFAULT_AGENTS_MD.replace("{name}", name)
}

/// Validate a project name. Rejects empty names, names containing `/` or `\`,
/// and names starting with `.`.
pub fn validate_project_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err(format!("Invalid project name: {}", name));
    }
    Ok(())
}

/// Create a `CLAUDE.md` symlink pointing to `agents.md` in the given directory,
/// if `agents.md` exists and `CLAUDE.md` does not.
pub fn ensure_claude_md_symlink(dir: &Path) -> Result<(), String> {
    let claude_md = dir.join("CLAUDE.md");
    let agents_md = dir.join("agents.md");
    if agents_md.exists() && !claude_md.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink("agents.md", &claude_md)
            .map_err(|e| format!("failed to create CLAUDE.md symlink: {}", e))?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("agents.md", &claude_md)
            .map_err(|e| format!("failed to create CLAUDE.md symlink: {}", e))?;
    }
    Ok(())
}
