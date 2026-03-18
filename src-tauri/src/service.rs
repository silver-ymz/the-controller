//! Service layer — shared business logic for Tauri commands and Axum handlers.
//!
//! Each public function in this module (and its submodules) encapsulates a
//! single unit of business logic. Both `commands.rs` (Tauri IPC) and
//! `server/` (Axum HTTP) delegate here, keeping the API surfaces thin.
//!
//! Errors are returned as [`crate::error::AppError`], which converts into
//! the appropriate response type for each API surface.
