use axum::{
    extract::{
        ws::{Message, WebSocket},
        Request, State as AxumState, WebSocketUpgrade,
    },
    http::{
        header::{HOST, ORIGIN},
        HeaderMap, StatusCode,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use the_controller_lib::{
    emitter::WsBroadcastEmitter,
    server_helpers::{ok_json, parse_uuid, ServerState},
    service,
    state::AppState,
    status_socket,
};

use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

mod requests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProjectSessionRequest {
        pub project_id: String,
        pub session_id: String,
    }
}

use requests::*;

fn report_startup_error(error: &std::io::Error) {
    tracing::error!("failed to start server: {error}");
    eprintln!("The Controller server failed to start: {error}");
}

fn get_port() -> u16 {
    std::env::var("CONTROLLER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3001)
}

fn get_bind_address() -> String {
    std::env::var("CONTROLLER_BIND").unwrap_or_else(|_| "0.0.0.0".to_string())
}

fn startup_messages(addr: &str, token: Option<&str>) -> (Option<String>, String) {
    match token.filter(|t| !t.is_empty()) {
        Some(_) => {
            (
                Some(format!(
                    "Server listening on http://{} (auth enabled; read CONTROLLER_AUTH_TOKEN from ~/.the-controller/server.env)",
                    addr
                )),
                format!("server listening on http://{} (auth enabled)", addr),
            )
        }
        None => (None, format!("server listening on http://{} (no auth)", addr)),
    }
}

async fn shutdown_signal(state: Arc<ServerState>) {
    let ctrl_c = async { tokio::signal::ctrl_c().await.unwrap() };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .unwrap()
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutting down");
    status_socket::cleanup();

    if let Ok(mut pty_manager) = state.app.pty_manager.lock() {
        let ids = pty_manager.session_ids();
        for id in ids {
            let _ = pty_manager.close_session(id);
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run_server().await {
        report_startup_error(&error);
        std::process::exit(1);
    }
}

async fn run_server() -> std::io::Result<()> {
    let base_dir = dirs::home_dir()
        .map(|h| h.join(".the-controller"))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let _log_guard = the_controller_lib::logging::init_backend_logging(&base_dir, true);

    let (emitter, ws_tx) = WsBroadcastEmitter::new();
    let app_state = Arc::new(AppState::new(emitter)?);

    // Start the status socket listener so Claude Code hooks can report session status
    status_socket::start_listener_with_state(app_state.clone());

    let state = Arc::new(ServerState {
        app: app_state,
        ws_tx,
    });

    let dist_dir = std::env::var("CONTROLLER_DIST_DIR").unwrap_or_else(|_| {
        let exe = std::env::current_exe().unwrap_or_default();
        exe.parent()
            .unwrap_or(Path::new("."))
            .join("dist")
            .to_string_lossy()
            .to_string()
    });

    let serve_dir =
        ServeDir::new(&dist_dir).fallback(ServeFile::new(format!("{}/index.html", dist_dir)));
    let auth_enabled = std::env::var("CONTROLLER_AUTH_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());

    let app = Router::new()
        .route("/api/list_projects", post(service::axum_list_projects))
        .route(
            "/api/check_onboarding",
            post(service::axum_check_onboarding),
        )
        .route(
            "/api/restore_sessions",
            post(service::axum_restore_sessions),
        )
        .route("/api/connect_session", post(service::axum_connect_session))
        .route("/api/load_project", post(service::axum_load_project))
        .route("/api/write_to_pty", post(service::axum_write_to_pty))
        .route("/api/send_raw_to_pty", post(service::axum_send_raw_to_pty))
        .route("/api/resize_pty", post(service::axum_resize_pty))
        .route("/api/close_session", post(service::axum_close_session))
        .route(
            "/api/create_session",
            post(service::axum_create_session_auto_id),
        )
        .route("/api/create_project", post(service::axum_create_project))
        .route("/api/delete_project", post(service::axum_delete_project))
        .route("/api/get_agents_md", post(service::axum_get_agents_md))
        .route(
            "/api/update_agents_md",
            post(service::axum_update_agents_md),
        )
        .route(
            "/api/set_initial_prompt",
            post(service::axum_set_initial_prompt),
        )
        .route(
            "/api/check_claude_cli",
            post(service::axum_check_claude_cli),
        )
        .route("/api/home_dir", post(service::axum_home_dir))
        .route(
            "/api/save_onboarding_config",
            post(service::axum_save_onboarding_config),
        )
        .route(
            "/api/log_frontend_error",
            post(service::axum_log_frontend_error),
        )
        .route(
            "/api/detect_project_type",
            post(service::axum_detect_project_type_blocking),
        )
        .route(
            "/api/get_deploy_credentials",
            post(service::axum_get_deploy_credentials_blocking),
        )
        .route(
            "/api/save_deploy_credentials",
            post(service::axum_save_deploy_credentials_blocking),
        )
        .route(
            "/api/is_deploy_provisioned",
            post(service::axum_is_deploy_provisioned_blocking),
        )
        .route("/api/deploy_project", post(service::axum_deploy_project))
        .route(
            "/api/list_deployed_services",
            post(service::axum_list_deployed_services),
        )
        .route(
            "/api/load_keybindings",
            post(service::axum_load_keybindings),
        )
        .route(
            "/api/copy_image_file_to_clipboard",
            post(copy_image_file_to_clipboard),
        )
        .route("/api/capture_app_screenshot", post(capture_app_screenshot))
        .route(
            "/api/start_voice_pipeline",
            post(service::axum_start_voice_pipeline),
        )
        .route(
            "/api/stop_voice_pipeline",
            post(service::axum_stop_voice_pipeline),
        )
        .route("/api/toggle_voice_pause", post(toggle_voice_pause))
        .route(
            "/api/load_terminal_theme",
            post(service::axum_load_terminal_theme_blocking),
        )
        .route(
            "/api/list_archived_projects",
            post(service::axum_list_archived_projects),
        )
        .route(
            "/api/generate_architecture",
            post(service::axum_generate_architecture),
        )
        .route("/api/merge_session_branch", post(merge_session_branch))
        .route(
            "/api/send_note_ai_chat",
            post(service::axum_send_note_ai_chat),
        )
        .route("/api/list_notes", post(service::axum_list_notes))
        .route("/api/read_note", post(service::axum_read_note))
        .route("/api/write_note", post(service::axum_write_note))
        .route("/api/create_note", post(service::axum_create_note))
        .route("/api/delete_note", post(service::axum_delete_note))
        .route("/api/rename_note", post(service::axum_rename_note))
        .route("/api/list_folders", post(service::axum_list_note_folders))
        .route("/api/create_folder", post(service::axum_create_note_folder))
        .route("/api/rename_folder", post(service::axum_rename_note_folder))
        .route("/api/delete_folder", post(service::axum_delete_note_folder))
        .route(
            "/api/commit_notes",
            post(service::axum_commit_pending_notes),
        )
        // GitHub issues
        .route(
            "/api/list_github_issues",
            post(service::axum_list_github_issues),
        )
        .route(
            "/api/list_assigned_issues",
            post(service::axum_list_assigned_issues),
        )
        .route(
            "/api/create_github_issue",
            post(service::axum_create_github_issue),
        )
        .route(
            "/api/generate_issue_body",
            post(service::axum_generate_issue_body),
        )
        .route(
            "/api/post_github_comment",
            post(service::axum_post_github_comment),
        )
        .route(
            "/api/add_github_label",
            post(service::axum_add_github_label),
        )
        .route(
            "/api/remove_github_label",
            post(service::axum_remove_github_label),
        )
        .route(
            "/api/close_github_issue",
            post(service::axum_close_github_issue),
        )
        .route(
            "/api/delete_github_issue",
            post(service::axum_delete_github_issue),
        )
        // Maintainer & auto-worker
        .route(
            "/api/configure_maintainer",
            post(service::axum_configure_maintainer),
        )
        .route(
            "/api/get_maintainer_status",
            post(service::axum_get_maintainer_status),
        )
        .route(
            "/api/get_maintainer_history",
            post(service::axum_get_maintainer_history_default),
        )
        .route(
            "/api/trigger_maintainer_check",
            post(service::axum_trigger_maintainer_check),
        )
        .route(
            "/api/clear_maintainer_reports",
            post(service::axum_clear_maintainer_reports),
        )
        .route(
            "/api/get_maintainer_issues",
            post(service::axum_get_maintainer_issues_for_project),
        )
        .route(
            "/api/get_maintainer_issue_detail",
            post(service::axum_get_maintainer_issue_detail_for_project),
        )
        .route(
            "/api/configure_auto_worker",
            post(service::axum_configure_auto_worker),
        )
        .route(
            "/api/get_auto_worker_queue",
            post(service::axum_get_auto_worker_queue),
        )
        .route(
            "/api/get_worker_reports",
            post(service::axum_get_worker_reports),
        )
        // Storage/git operations
        .route(
            "/api/get_session_commits",
            post(service::axum_get_session_commits),
        )
        .route(
            "/api/save_session_prompt",
            post(service::axum_save_session_prompt),
        )
        .route(
            "/api/list_project_prompts",
            post(service::axum_list_project_prompts),
        )
        .route("/api/get_repo_head", post(service::axum_get_repo_head))
        .route(
            "/api/get_session_token_usage",
            post(service::axum_get_session_token_usage),
        )
        // Directory listing
        .route(
            "/api/list_directories_at",
            post(service::axum_list_directories_at_safe),
        )
        .route(
            "/api/list_root_directories",
            post(service::axum_list_root_directories),
        )
        .route(
            "/api/generate_project_names",
            post(service::axum_generate_project_names),
        )
        // Scaffold
        .route(
            "/api/scaffold_project",
            post(service::axum_scaffold_project),
        )
        // Session management
        .route("/api/stage_session", post(stage_session))
        .route("/api/unstage_session", post(service::axum_unstage_session))
        .route(
            "/api/submit_secure_env_value",
            post(service::axum_submit_secure_env_value),
        )
        .route(
            "/api/cancel_secure_env_request",
            post(service::axum_cancel_secure_env_request),
        )
        // Notes (additional)
        .route("/api/save_note_image", post(save_note_image))
        .route(
            "/api/resolve_note_asset_path",
            post(service::axum_resolve_note_asset_path),
        )
        .route("/api/duplicate_note", post(service::axum_duplicate_note))
        // Auth/login
        .route(
            "/api/start_claude_login",
            post(service::axum_start_claude_login),
        )
        .route(
            "/api/stop_claude_login",
            post(service::axum_stop_claude_login),
        )
        .route("/ws", get(ws_upgrade))
        .fallback_service(serve_dir)
        .layer(middleware::from_fn(auth_middleware))
        .with_state(state.clone());
    let app = if auth_enabled.is_some() {
        app.layer(CorsLayer::permissive())
    } else {
        app
    };

    let port = get_port();
    let bind = get_bind_address();
    let addr = format!("{}:{}", bind, port);
    let (stdout_message, log_message) = startup_messages(&addr, auth_enabled.as_deref());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    if let Some(message) = stdout_message {
        println!("{}", message);
    }
    tracing::info!("{}", log_message);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await
        .map_err(std::io::Error::other)
}

// --- Auth middleware ---

async fn auth_middleware(req: Request, next: Next) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    if !is_api_or_ws_path(path) {
        return Ok(next.run(req).await);
    }

    let token = match std::env::var("CONTROLLER_AUTH_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            if request_origin_allowed(&req) {
                return Ok(next.run(req).await);
            }
            return Err(StatusCode::FORBIDDEN);
        }
    };

    // Check Authorization header or query param
    let authorized = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.strip_prefix("Bearer ").unwrap_or(v) == token)
        .unwrap_or(false)
        || req
            .uri()
            .query()
            .and_then(|query| decoded_query_param(query, "token"))
            .map(|candidate| candidate == token)
            .unwrap_or(false);

    if authorized {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn is_api_or_ws_path(path: &str) -> bool {
    path.starts_with("/api/") || path == "/ws"
}

fn decoded_query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .and_then(percent_decode)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hi = decode_hex(bytes[index + 1])?;
                let lo = decode_hex(bytes[index + 2])?;
                decoded.push((hi << 4) | lo);
                index += 3;
            }
            b'%' => return None,
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded).ok()
}

fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn request_origin_allowed(req: &Request) -> bool {
    match request_origin(req.headers()) {
        None => true,
        Some("null") => false,
        Some(origin) => expected_request_origin(req.headers()).as_deref() == Some(origin),
    }
}

fn request_origin(headers: &HeaderMap) -> Option<&str> {
    headers.get(ORIGIN).and_then(|value| value.to_str().ok())
}

fn expected_request_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(HOST)?.to_str().ok()?;
    let proto = forwarded_proto(headers).unwrap_or("http");
    Some(format!("{proto}://{host}"))
}

fn forwarded_proto(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

// --- Route handlers ---

// --- Desktop-only stubs (return NOT_IMPLEMENTED gracefully) ---

async fn copy_image_file_to_clipboard() -> Result<Json<Value>, (StatusCode, String)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "copy_image_file_to_clipboard is not available in server mode".to_string(),
    ))
}

async fn capture_app_screenshot() -> Result<Json<Value>, (StatusCode, String)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "capture_app_screenshot is not available in server mode".to_string(),
    ))
}

async fn merge_session_branch(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_uuid = parse_uuid(&req.session_id)?;

    const OVERALL_TIMEOUT_SECS: u64 = 600; // 10 minutes

    let merge_result = tokio::time::timeout(
        std::time::Duration::from_secs(OVERALL_TIMEOUT_SECS),
        service::merge_session_branch(&state.app, project_uuid, session_uuid, true),
    )
    .await;

    match merge_result {
        Ok(inner) => {
            let resp = inner.map_err(<(StatusCode, String)>::from)?;
            ok_json(resp)
        }
        Err(_) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            format!("Merge timed out after {} seconds", OVERALL_TIMEOUT_SECS),
        )),
    }
}

// --- Notes (binary data) ---

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveNoteImageRequest {
    folder: String,
    image_bytes: String, // base64-encoded
    extension: String,
}

async fn save_note_image(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<SaveNoteImageRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    use base64::Engine;
    let image_bytes = base64::engine::general_purpose::STANDARD
        .decode(&req.image_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid base64: {e}")))?;
    let filename = tokio::task::spawn_blocking(move || {
        service::save_note_image(&state.app, &req.folder, &image_bytes, &req.extension)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("task failed: {e}"),
        )
    })?
    .map_err(<(StatusCode, String)>::from)?;
    ok_json(filename)
}

// --- Voice (response shape) ---

/// Hand-written because the old API returns `{"paused": bool}` but the
/// macro-generated handler would return a bare `bool`.
async fn toggle_voice_pause(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let paused = service::toggle_voice_pause(&state.app)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(serde_json::json!({ "paused": paused }))
}

// --- Session Management ---

async fn stage_session() -> Result<Json<Value>, (StatusCode, String)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "stage_session requires AppHandle and is not available in server mode".to_string(),
    ))
}

// --- WebSocket ---

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    AxumState(state): AxumState<Arc<ServerState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state.ws_tx.subscribe()))
}

async fn handle_ws(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    loop {
        let msg = match rx.recv().await {
            Ok(msg) => msg,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };

        if socket.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{auth_middleware, handle_ws, startup_messages};
    use axum::{
        extract::{ws::WebSocketUpgrade, State as AxumState},
        middleware,
        response::IntoResponse,
        routing::{get, post},
        Router,
    };
    use futures_util::StreamExt;
    use once_cell::sync::Lazy;
    use reqwest::StatusCode;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use the_controller_lib::server_helpers::ServerState;
    use tokio::net::TcpListener;
    use tokio::sync::broadcast;
    use tokio::time::{timeout, Duration};
    use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

    use tempfile::TempDir;
    use the_controller_lib::{emitter::NoopEmitter, state::AppState, storage::Storage};

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn remove(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, original }
        }

        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn source_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn percent_encode_component(value: &str) -> String {
        let mut encoded = String::new();
        for byte in value.bytes() {
            if matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~') {
                encoded.push(byte as char);
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
        encoded
    }

    async fn spawn_test_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let addr = listener
            .local_addr()
            .expect("test listener should expose local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });
        (format!("http://{}", addr), handle)
    }

    fn extract_desktop_commands(source: &str) -> BTreeSet<String> {
        let invoke_list = source
            .split("tauri::generate_handler![")
            .nth(1)
            .and_then(|section| section.split("])").next())
            .expect("desktop invoke handler list should exist");

        invoke_list
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim().trim_end_matches(',');
                trimmed
                    .strip_prefix("commands::")
                    .or_else(|| trimmed.strip_prefix("deploy::commands::"))
                    .map(ToOwned::to_owned)
            })
            .collect()
    }

    fn extract_server_routes(source: &str) -> BTreeSet<String> {
        source
            .split("#[cfg(test)]")
            .next()
            .expect("server source should have a non-test section")
            .split(".route(")
            .skip(1)
            .filter_map(|segment| {
                let start = segment.find('"')?;
                let after_quote = &segment[start + 1..];
                let end = after_quote.find('"')?;
                let path = &after_quote[..end];
                path.strip_prefix("/api/").map(ToOwned::to_owned)
            })
            .collect()
    }

    #[test]
    fn server_routes_cover_desktop_command_surface() {
        let root = source_root();
        let lib_rs = fs::read_to_string(root.join("lib.rs")).expect("should read lib.rs");
        let server_rs =
            fs::read_to_string(root.join("server/main.rs")).expect("should read server.rs");

        let desktop_commands = extract_desktop_commands(&lib_rs);
        let server_routes = extract_server_routes(&server_rs);
        let allowed_server_only = BTreeSet::from([String::from("list_archived_projects")]);

        let missing: Vec<_> = desktop_commands
            .difference(&server_routes)
            .cloned()
            .collect();
        let unexpected_server_only: Vec<_> = server_routes
            .difference(&desktop_commands)
            .filter(|name| !allowed_server_only.contains(*name))
            .cloned()
            .collect();

        assert!(
            missing.is_empty(),
            "server router is missing desktop commands: {:?}",
            missing
        );
        assert!(
            unexpected_server_only.is_empty(),
            "server router has unexpected command routes: {:?}",
            unexpected_server_only
        );
    }

    #[test]
    fn authenticated_startup_message_does_not_print_raw_token() {
        let addr = "127.0.0.1:3001";
        let token = "super-secret-token";

        let (stdout_message, log_message) = startup_messages(addr, Some(token));

        let stdout_message = stdout_message.expect("authenticated startup should print guidance");
        assert!(stdout_message.contains(addr));
        assert!(
            !stdout_message.contains(token),
            "stdout should not contain raw auth token"
        );
        assert!(
            stdout_message.contains("server.env"),
            "stdout should direct operators to the config file"
        );

        assert!(log_message.contains(addr));
        assert!(!log_message.contains(token));
    }

    #[tokio::test]
    async fn unauthenticated_cross_origin_api_requests_are_forbidden() {
        let _env_lock = ENV_LOCK.lock().expect("env lock should not be poisoned");
        let _auth_guard = EnvGuard::remove("CONTROLLER_AUTH_TOKEN");
        let app = Router::new()
            .route("/api/ping", post(|| async { StatusCode::OK }))
            .layer(middleware::from_fn(auth_middleware));
        let (base_url, server_handle) = spawn_test_server(app).await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{base_url}/api/ping"))
            .header("Origin", "https://evil.example")
            .send()
            .await
            .expect("cross-origin request should complete");

        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
        server_handle.abort();
    }

    #[tokio::test]
    async fn unauthenticated_same_origin_api_requests_are_allowed() {
        let _env_lock = ENV_LOCK.lock().expect("env lock should not be poisoned");
        let _auth_guard = EnvGuard::remove("CONTROLLER_AUTH_TOKEN");
        let app = Router::new()
            .route("/api/ping", post(|| async { StatusCode::OK }))
            .layer(middleware::from_fn(auth_middleware));
        let (base_url, server_handle) = spawn_test_server(app).await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{base_url}/api/ping"))
            .header("Origin", &base_url)
            .send()
            .await
            .expect("same-origin request should complete");

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_auth_accepts_percent_encoded_token_query() {
        let _env_lock = ENV_LOCK.lock().expect("env lock should not be poisoned");

        async fn test_ws_upgrade(
            ws: WebSocketUpgrade,
            AxumState(state): AxumState<Arc<ServerState>>,
        ) -> impl IntoResponse {
            ws.write_buffer_size(0)
                .on_upgrade(move |socket| handle_ws(socket, state.ws_tx.subscribe()))
        }

        let token = "abc/+%=&token";
        let _auth_guard = EnvGuard::set("CONTROLLER_AUTH_TOKEN", token);
        let temp_dir = TempDir::new().expect("temp dir should be created");
        let app_state = Arc::new(
            AppState::from_storage(
                Storage::new(temp_dir.path().to_path_buf()),
                NoopEmitter::new(),
            )
            .expect("test app state should initialize"),
        );
        let (ws_tx, _) = broadcast::channel(8);
        let state = Arc::new(ServerState {
            app: app_state,
            ws_tx: ws_tx.clone(),
        });
        let app = Router::new()
            .route("/api/ping", post(|| async { StatusCode::OK }))
            .route("/ws", get(test_ws_upgrade))
            .layer(middleware::from_fn(auth_middleware))
            .with_state(state);
        let (base_url, server_handle) = spawn_test_server(app).await;

        let client = reqwest::Client::new();
        let api_response = client
            .post(format!("{base_url}/api/ping"))
            .bearer_auth(token)
            .send()
            .await
            .expect("authenticated API request should complete");
        assert_eq!(api_response.status(), reqwest::StatusCode::OK);

        let ws_url = format!(
            "{}/ws?token={}",
            base_url.replacen("http", "ws", 1),
            percent_encode_component(token)
        );
        let (mut socket, _) = connect_async(&ws_url)
            .await
            .expect("websocket client should connect with percent-encoded token");

        let expected = "authenticated".to_string();
        ws_tx
            .send(expected.clone())
            .expect("event should be broadcast");
        let received = timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("authenticated websocket should receive an event before timeout")
            .expect("socket should remain open after auth")
            .expect("frame should decode");
        assert_eq!(
            received.into_text().expect("frame should be text"),
            expected
        );

        server_handle.abort();
    }

    #[tokio::test]
    async fn websocket_client_recovers_after_broadcast_lag() {
        async fn test_ws_upgrade(
            ws: WebSocketUpgrade,
            AxumState(state): AxumState<Arc<ServerState>>,
        ) -> impl IntoResponse {
            ws.write_buffer_size(0)
                .on_upgrade(move |socket| handle_ws(socket, state.ws_tx.subscribe()))
        }

        let temp_dir = TempDir::new().expect("temp dir should be created");
        let app_state = Arc::new(
            AppState::from_storage(
                Storage::new(temp_dir.path().to_path_buf()),
                NoopEmitter::new(),
            )
            .expect("test app state should initialize"),
        );
        let (ws_tx, _) = broadcast::channel(2);
        let state = Arc::new(ServerState {
            app: app_state,
            ws_tx: ws_tx.clone(),
        });
        let app = Router::new()
            .route("/ws", get(test_ws_upgrade))
            .with_state(state);
        let (base_url, server_handle) = spawn_test_server(app).await;

        let ws_url = format!("{}/ws", base_url.replacen("http", "ws", 1));
        let (mut socket, _) = connect_async(&ws_url)
            .await
            .expect("websocket client should connect");

        ws_tx
            .send("ready".to_string())
            .expect("ready event should be broadcast");
        let ready = timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("ready event should arrive before timeout")
            .expect("socket should remain open before lag")
            .expect("ready frame should decode");
        assert_eq!(
            ready.into_text().expect("ready frame should be text"),
            "ready"
        );

        let flood_payload = "x".repeat(256 * 1024);
        for index in 0..64 {
            ws_tx
                .send(format!("flood-{index}:{flood_payload}"))
                .expect("flood event should be broadcast");
        }
        let recovery = "recovery".to_string();
        ws_tx
            .send(recovery.clone())
            .expect("recovery event should be broadcast");

        let recovered = timeout(Duration::from_secs(5), async {
            loop {
                match socket.next().await {
                    Some(Ok(WsMessage::Text(text))) if text == recovery => return true,
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => {
                        panic!("websocket should stay connected after lag: {error}")
                    }
                    None => return false,
                }
            }
        })
        .await
        .expect("recovery event should arrive before timeout");

        assert!(
            recovered,
            "socket closed after lag instead of receiving later events"
        );
        server_handle.abort();
    }

    #[test]
    fn parse_uuid_accepts_valid_uuid() {
        let id = uuid::Uuid::new_v4();
        let parsed = super::parse_uuid(&id.to_string());
        assert_eq!(parsed.unwrap(), id);
    }

    #[test]
    fn parse_uuid_rejects_invalid_input_with_bad_request() {
        let result = super::parse_uuid("not-a-uuid");
        let (status, _msg) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
