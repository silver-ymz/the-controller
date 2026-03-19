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
    config, deploy,
    emitter::WsBroadcastEmitter,
    models, note_ai_chat,
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

    // --- Reusable single-field structs ---

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProjectIdRequest {
        pub project_id: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RepoPathRequest {
        pub repo_path: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SessionIdRequest {
        pub session_id: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ProjectSessionRequest {
        pub project_id: String,
        pub session_id: String,
    }

    // --- Handler-specific structs ---

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ConnectSessionRequest {
        pub session_id: String,
        #[serde(default = "default_rows")]
        pub rows: u16,
        #[serde(default = "default_cols")]
        pub cols: u16,
    }

    fn default_rows() -> u16 {
        24
    }
    fn default_cols() -> u16 {
        80
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct LoadProjectRequest {
        pub name: String,
        pub repo_path: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct WriteToPtyRequest {
        pub session_id: String,
        pub data: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ResizePtyRequest {
        pub session_id: String,
        #[serde(default = "default_rows")]
        pub rows: u16,
        #[serde(default = "default_cols")]
        pub cols: u16,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CloseSessionRequest {
        pub project_id: String,
        pub session_id: String,
        #[serde(default)]
        pub delete_worktree: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateSessionRequest {
        pub project_id: String,
        #[serde(default = "default_kind")]
        pub kind: String,
        #[serde(default)]
        pub background: bool,
        pub initial_prompt: Option<String>,
        pub github_issue: Option<models::GithubIssue>,
    }

    fn default_kind() -> String {
        "claude".to_string()
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateProjectRequest {
        pub name: String,
        pub repo_path: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DeleteProjectRequest {
        pub project_id: String,
        #[serde(default)]
        pub delete_repo: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct UpdateAgentsMdRequest {
        pub project_id: String,
        pub content: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SetInitialPromptRequest {
        pub project_id: String,
        pub session_id: String,
        pub prompt: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SaveOnboardingConfigRequest {
        pub projects_root: String,
        #[serde(default)]
        pub default_provider: config::ConfigDefaultProvider,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct LogFrontendErrorRequest {
        #[serde(default)]
        pub message: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SaveDeployCredentialsRequest {
        pub credentials: deploy::credentials::DeployCredentials,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DeployProjectRequest {
        pub request: deploy::commands::DeployRequest,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SendNoteAiChatRequest {
        pub note_content: String,
        pub selected_text: String,
        pub prompt: String,
        #[serde(default)]
        pub conversation_history: Vec<note_ai_chat::NoteAiChatMessage>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FolderRequest {
        pub folder: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FolderFilenameRequest {
        pub folder: String,
        pub filename: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct WriteNoteRequest {
        pub folder: String,
        pub filename: String,
        pub content: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateNoteRequest {
        pub folder: String,
        pub title: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RenameNoteRequest {
        pub folder: String,
        pub old_name: String,
        pub new_name: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct NameRequest {
        pub name: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RenameFolderRequest {
        pub old_name: String,
        pub new_name: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DeleteFolderRequest {
        pub name: String,
        #[serde(default)]
        pub force: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RepoPathIssueNumberRequest {
        pub repo_path: String,
        pub issue_number: u64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CreateGithubIssueRequest {
        pub repo_path: String,
        pub title: String,
        pub body: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct TitleRequest {
        pub title: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PostGithubCommentRequest {
        pub repo_path: String,
        pub issue_number: u64,
        pub body: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AddGithubLabelRequest {
        pub repo_path: String,
        pub issue_number: u64,
        pub label: String,
        pub description: Option<String>,
        pub color: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct RemoveGithubLabelRequest {
        pub repo_path: String,
        pub issue_number: u64,
        pub label: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CloseGithubIssueRequest {
        pub repo_path: String,
        pub issue_number: u64,
        pub comment: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ConfigureMaintainerRequest {
        pub project_id: String,
        #[serde(default)]
        pub enabled: bool,
        #[serde(default = "default_interval_minutes")]
        pub interval_minutes: u64,
        pub github_repo: Option<String>,
    }

    fn default_interval_minutes() -> u64 {
        30
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct GetMaintainerIssueDetailRequest {
        pub project_id: String,
        pub issue_number: u64,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ConfigureAutoWorkerRequest {
        pub project_id: String,
        #[serde(default)]
        pub enabled: bool,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PathRequest {
        pub path: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct DescriptionRequest {
        pub description: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SubmitSecureEnvValueRequest {
        pub request_id: String,
        pub value: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct CancelSecureEnvRequestRequest {
        pub request_id: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SaveNoteImageRequest {
        pub folder: String,
        pub extension: String,
        pub image_bytes: Vec<u8>,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ResolveNoteAssetPathRequest {
        pub folder: String,
        pub relative_path: String,
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
        .route("/api/list_projects", post(list_projects))
        .route("/api/check_onboarding", post(check_onboarding))
        .route("/api/restore_sessions", post(restore_sessions))
        .route("/api/connect_session", post(connect_session))
        .route("/api/load_project", post(load_project))
        .route("/api/write_to_pty", post(write_to_pty))
        .route("/api/send_raw_to_pty", post(send_raw_to_pty))
        .route("/api/resize_pty", post(resize_pty))
        .route("/api/close_session", post(close_session))
        .route("/api/create_session", post(create_session))
        .route("/api/create_project", post(create_project))
        .route("/api/delete_project", post(delete_project))
        .route("/api/get_agents_md", post(get_agents_md))
        .route("/api/update_agents_md", post(update_agents_md))
        .route("/api/set_initial_prompt", post(set_initial_prompt))
        .route("/api/check_claude_cli", post(check_claude_cli))
        .route("/api/home_dir", post(home_dir))
        .route("/api/save_onboarding_config", post(save_onboarding_config))
        .route("/api/log_frontend_error", post(log_frontend_error))
        .route("/api/detect_project_type", post(detect_project_type))
        .route("/api/get_deploy_credentials", post(get_deploy_credentials))
        .route(
            "/api/save_deploy_credentials",
            post(save_deploy_credentials),
        )
        .route("/api/is_deploy_provisioned", post(is_deploy_provisioned))
        .route("/api/deploy_project", post(deploy_project))
        .route("/api/list_deployed_services", post(list_deployed_services))
        .route("/api/load_keybindings", post(load_keybindings))
        .route(
            "/api/copy_image_file_to_clipboard",
            post(copy_image_file_to_clipboard),
        )
        .route("/api/capture_app_screenshot", post(capture_app_screenshot))
        .route("/api/start_voice_pipeline", post(start_voice_pipeline))
        .route("/api/stop_voice_pipeline", post(stop_voice_pipeline))
        .route("/api/toggle_voice_pause", post(toggle_voice_pause))
        .route("/api/load_terminal_theme", post(load_terminal_theme))
        .route("/api/list_archived_projects", post(list_archived_projects))
        .route("/api/generate_architecture", post(generate_architecture))
        .route("/api/merge_session_branch", post(merge_session_branch))
        .route("/api/send_note_ai_chat", post(send_note_ai_chat))
        .route("/api/list_notes", post(api_list_notes))
        .route("/api/read_note", post(api_read_note))
        .route("/api/write_note", post(api_write_note))
        .route("/api/create_note", post(api_create_note))
        .route("/api/delete_note", post(api_delete_note))
        .route("/api/rename_note", post(api_rename_note))
        .route("/api/list_folders", post(api_list_folders))
        .route("/api/create_folder", post(api_create_folder))
        .route("/api/rename_folder", post(api_rename_folder))
        .route("/api/delete_folder", post(api_delete_folder))
        .route("/api/commit_notes", post(api_commit_notes))
        // GitHub issues
        .route("/api/list_github_issues", post(list_github_issues))
        .route("/api/list_assigned_issues", post(list_assigned_issues))
        .route("/api/create_github_issue", post(create_github_issue))
        .route("/api/generate_issue_body", post(generate_issue_body))
        .route("/api/post_github_comment", post(post_github_comment))
        .route("/api/add_github_label", post(add_github_label))
        .route("/api/remove_github_label", post(remove_github_label))
        .route("/api/close_github_issue", post(close_github_issue))
        .route("/api/delete_github_issue", post(delete_github_issue))
        // Maintainer & auto-worker
        .route("/api/configure_maintainer", post(configure_maintainer))
        .route("/api/get_maintainer_status", post(get_maintainer_status))
        .route("/api/get_maintainer_history", post(get_maintainer_history))
        .route(
            "/api/trigger_maintainer_check",
            post(trigger_maintainer_check),
        )
        .route(
            "/api/clear_maintainer_reports",
            post(clear_maintainer_reports),
        )
        .route("/api/get_maintainer_issues", post(get_maintainer_issues))
        .route(
            "/api/get_maintainer_issue_detail",
            post(get_maintainer_issue_detail),
        )
        .route("/api/configure_auto_worker", post(configure_auto_worker))
        .route("/api/get_auto_worker_queue", post(get_auto_worker_queue))
        .route("/api/get_worker_reports", post(get_worker_reports))
        // Storage/git operations
        .route("/api/get_session_commits", post(get_session_commits))
        .route("/api/save_session_prompt", post(save_session_prompt))
        .route("/api/list_project_prompts", post(list_project_prompts))
        .route("/api/get_repo_head", post(get_repo_head))
        .route(
            "/api/get_session_token_usage",
            post(get_session_token_usage),
        )
        // Directory listing
        .route("/api/list_directories_at", post(list_directories_at))
        .route("/api/list_root_directories", post(list_root_directories))
        .route("/api/generate_project_names", post(generate_project_names))
        // Scaffold
        .route("/api/scaffold_project", post(scaffold_project))
        // Session management
        .route("/api/stage_session", post(stage_session))
        .route("/api/unstage_session", post(unstage_session))
        .route(
            "/api/submit_secure_env_value",
            post(submit_secure_env_value),
        )
        .route(
            "/api/cancel_secure_env_request",
            post(cancel_secure_env_request),
        )
        // Notes (additional)
        .route("/api/save_note_image", post(api_save_note_image))
        .route(
            "/api/resolve_note_asset_path",
            post(api_resolve_note_asset_path),
        )
        .route("/api/duplicate_note", post(api_duplicate_note))
        // Auth/login
        .route("/api/start_claude_login", post(start_claude_login))
        .route("/api/stop_claude_login", post(stop_claude_login))
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

// --- Shared handler utilities ---

/// Run a blocking closure on a threadpool and map both join-failure and
/// `AppError` to Axum's error tuple.
///
/// Usage (clone only the Arcs you actually need):
/// ```ignore
/// let result = spawn_blocking_handler!({
///     let foo = state.app.foo.clone();
///     move || service::do_thing(&foo, arg)
/// })?;
/// ```
macro_rules! spawn_blocking_handler {
    ($closure:expr) => {{
        tokio::task::spawn_blocking($closure)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Task failed: {e}"),
                )
            })?
            .map_err(<(StatusCode, String)>::from)
    }};
}

// --- Route handlers ---

async fn list_projects(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let inventory = service::list_projects(&state.app).map_err(<(StatusCode, String)>::from)?;
    ok_json(inventory)
}

async fn check_onboarding(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = service::check_onboarding(&state.app).map_err(<(StatusCode, String)>::from)?;
    ok_json(cfg)
}

async fn restore_sessions(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::restore_sessions(&state.app).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn connect_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ConnectSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.session_id)?;

    service::connect_session(&state.app, id, req.rows, req.cols)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn load_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<LoadProjectRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project = service::load_project(&state.app, &req.name, &req.repo_path)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(project)
}

async fn write_to_pty(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<WriteToPtyRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.session_id)?;
    service::write_to_pty(&state.app, id, req.data.as_bytes())
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn send_raw_to_pty(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<WriteToPtyRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.session_id)?;
    service::send_raw_to_pty(&state.app, id, req.data.as_bytes())
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn resize_pty(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ResizePtyRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.session_id)?;
    service::resize_pty(&state.app, id, req.rows, req.cols)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn close_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<CloseSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_uuid = parse_uuid(&req.session_id)?;

    service::close_session(&state.app, project_uuid, session_uuid, req.delete_worktree)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn create_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_id = uuid::Uuid::new_v4();

    let app = state.app.clone();
    let kind = req.kind;
    let github_issue = req.github_issue;
    let background = req.background;
    let initial_prompt = req.initial_prompt;

    let result = spawn_blocking_handler!(move || {
        service::create_session(
            &app,
            project_uuid,
            session_id,
            &kind,
            github_issue,
            background,
            initial_prompt,
        )
    })?;

    Ok(Json(Value::String(result)))
}

async fn generate_architecture(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<RepoPathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let app = state.app.clone();
    let result =
        spawn_blocking_handler!(move || { service::generate_architecture(&app, &req.repo_path) })?;
    ok_json(result)
}

async fn create_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project = service::create_project(&state.app, &req.name, &req.repo_path)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(project)
}

async fn delete_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<DeleteProjectRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.project_id)?;

    service::delete_project(&state.app, id, req.delete_repo)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn get_agents_md(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.project_id)?;
    let content = service::get_agents_md(&state.app, id).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(content)))
}

async fn update_agents_md(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<UpdateAgentsMdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.project_id)?;
    service::update_agents_md(&state.app, id, &req.content)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn set_initial_prompt(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<SetInitialPromptRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_uuid = parse_uuid(&req.session_id)?;

    service::set_initial_prompt(&state.app, project_uuid, session_uuid, req.prompt)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn check_claude_cli() -> Result<Json<Value>, (StatusCode, String)> {
    // service::check_claude_cli is infallible (returns String, not Result).
    // Wrap in Ok::<_, AppError> so spawn_blocking_handler! gets a Result<T, AppError>.
    let result = spawn_blocking_handler!(|| Ok::<_, the_controller_lib::error::AppError>(
        service::check_claude_cli()
    ))?;
    Ok(Json(Value::String(result)))
}

async fn home_dir() -> Result<Json<Value>, (StatusCode, String)> {
    let path = service::home_dir().map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(path)))
}

async fn save_onboarding_config(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<SaveOnboardingConfigRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::save_onboarding_config(&state.app, &req.projects_root, Some(req.default_provider))
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn log_frontend_error(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<LogFrontendErrorRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::log_frontend_error(&state.app, &req.message);
    Ok(Json(Value::Null))
}

async fn detect_project_type(
    Json(req): Json<RepoPathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = req.repo_path;
    let result =
        spawn_blocking_handler!(move || { service::detect_project_type_blocking(&repo_path) })?;
    ok_json(result)
}

async fn get_deploy_credentials() -> Result<Json<Value>, (StatusCode, String)> {
    let result = spawn_blocking_handler!(service::get_deploy_credentials_blocking)?;
    ok_json(result)
}

async fn save_deploy_credentials(
    Json(req): Json<SaveDeployCredentialsRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    spawn_blocking_handler!(move || service::save_deploy_credentials_blocking(req.credentials))?;
    Ok(Json(Value::Null))
}

async fn is_deploy_provisioned() -> Result<Json<Value>, (StatusCode, String)> {
    let provisioned = spawn_blocking_handler!(service::is_deploy_provisioned_blocking)?;
    Ok(Json(Value::Bool(provisioned)))
}

async fn deploy_project(
    Json(req): Json<DeployProjectRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let result = service::deploy_project(req.request)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(result)
}

async fn list_deployed_services() -> Result<Json<Value>, (StatusCode, String)> {
    let services = service::list_deployed_services()
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Array(services)))
}

async fn load_keybindings(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let result = service::load_keybindings(&state.app).map_err(<(StatusCode, String)>::from)?;
    ok_json(result)
}

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

async fn start_voice_pipeline(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::start_voice_pipeline(&state.app)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn stop_voice_pipeline(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::stop_voice_pipeline(&state.app)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn toggle_voice_pause(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let paused = service::toggle_voice_pause(&state.app)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::json!({ "paused": paused })))
}

async fn load_terminal_theme(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let app = state.app.clone();
    let theme = spawn_blocking_handler!(move || service::load_terminal_theme_blocking(&app))?;
    ok_json(theme)
}

async fn list_archived_projects(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let filtered =
        service::list_archived_projects(&state.app).map_err(<(StatusCode, String)>::from)?;
    ok_json(filtered)
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

async fn send_note_ai_chat(
    Json(req): Json<SendNoteAiChatRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let response = service::send_note_ai_chat(
        req.note_content,
        req.selected_text,
        req.conversation_history,
        req.prompt,
    )
    .await
    .map_err(<(StatusCode, String)>::from)?;

    ok_json(response)
}
// --- Notes ---

async fn api_list_notes(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<FolderRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let entries =
        service::list_notes(&state.app, &req.folder).map_err(<(StatusCode, String)>::from)?;
    ok_json(entries)
}

async fn api_read_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<FolderFilenameRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let content = service::read_note(&state.app, &req.folder, &req.filename)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(content)
}

async fn api_write_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<WriteNoteRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::write_note(&state.app, &req.folder, &req.filename, &req.content)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_create_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<CreateNoteRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let filename = service::create_note(&state.app, &req.folder, &req.title)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(filename)
}

async fn api_delete_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<FolderFilenameRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::delete_note(&state.app, &req.folder, &req.filename)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_rename_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<RenameNoteRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let filename = service::rename_note(&state.app, &req.folder, &req.old_name, &req.new_name)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(filename)
}

async fn api_list_folders(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folders = service::list_note_folders(&state.app).map_err(<(StatusCode, String)>::from)?;
    ok_json(folders)
}

async fn api_create_folder(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<NameRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::create_note_folder(&state.app, &req.name).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_rename_folder(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<RenameFolderRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::rename_note_folder(&state.app, &req.old_name, &req.new_name)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_delete_folder(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<DeleteFolderRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::delete_note_folder(&state.app, &req.name, req.force)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_commit_notes(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let committed =
        service::commit_pending_notes(&state.app).map_err(<(StatusCode, String)>::from)?;
    ok_json(committed)
}

// --- GitHub Issues ---

async fn list_github_issues(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<RepoPathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let issues = service::list_github_issues(&state.app, &req.repo_path)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(issues)
}

async fn list_assigned_issues(
    Json(req): Json<RepoPathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let assigned = service::list_assigned_issues(&req.repo_path)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(assigned)
}

async fn create_github_issue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<CreateGithubIssueRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let issue = service::create_github_issue(&state.app, &req.repo_path, &req.title, &req.body)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(issue)
}

async fn generate_issue_body(
    Json(req): Json<TitleRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let body = service::generate_issue_body(&req.title)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(body)))
}

async fn post_github_comment(
    Json(req): Json<PostGithubCommentRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::post_github_comment(&req.repo_path, req.issue_number, &req.body)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn add_github_label(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<AddGithubLabelRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::add_github_label(
        &state.app,
        &req.repo_path,
        req.issue_number,
        &req.label,
        req.description.as_deref(),
        req.color.as_deref(),
    )
    .await
    .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn remove_github_label(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<RemoveGithubLabelRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::remove_github_label(&state.app, &req.repo_path, req.issue_number, &req.label)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn close_github_issue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<CloseGithubIssueRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::close_github_issue(&state.app, &req.repo_path, req.issue_number, &req.comment)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn delete_github_issue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<RepoPathIssueNumberRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::delete_github_issue(&state.app, &req.repo_path, req.issue_number)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

// --- Maintainer & Auto-Worker ---

async fn configure_maintainer(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ConfigureMaintainerRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    service::configure_maintainer(
        &state.app,
        project_uuid,
        req.enabled,
        req.interval_minutes,
        req.github_repo,
    )
    .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn get_maintainer_status(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let log = service::get_maintainer_status(&state.app, project_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(log)
}

async fn get_maintainer_history(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let logs = service::get_maintainer_history(&state.app, project_uuid, 20)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(logs)
}

async fn trigger_maintainer_check(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let log = service::trigger_maintainer_check(&state.app, project_uuid)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(log)
}

async fn clear_maintainer_reports(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    service::clear_maintainer_reports(&state.app, project_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn get_maintainer_issues(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let issues = service::get_maintainer_issues_for_project(&state.app, project_uuid)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(issues)
}

async fn get_maintainer_issue_detail(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<GetMaintainerIssueDetailRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let detail = service::get_maintainer_issue_detail_for_project(
        &state.app,
        project_uuid,
        req.issue_number as u32,
    )
    .await
    .map_err(<(StatusCode, String)>::from)?;
    ok_json(detail)
}

async fn configure_auto_worker(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ConfigureAutoWorkerRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    service::configure_auto_worker(&state.app, project_uuid, req.enabled)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn get_auto_worker_queue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let queue = service::get_auto_worker_queue(&state.app, project_uuid)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(queue)
}

async fn get_worker_reports(
    Json(req): Json<RepoPathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let reports = service::get_worker_reports(&req.repo_path)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(reports)
}

// --- Storage/Git Operations ---

async fn get_session_commits(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_uuid = parse_uuid(&req.session_id)?;
    let app = state.app.clone();
    let commits = spawn_blocking_handler!(move || {
        service::get_session_commits(&app, project_uuid, session_uuid)
    })?;
    ok_json(commits)
}

async fn save_session_prompt(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_uuid = parse_uuid(&req.session_id)?;
    service::save_session_prompt(&state.app, project_uuid, session_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn list_project_prompts(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let prompts = service::list_project_prompts(&state.app, project_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(prompts)
}

async fn get_repo_head(
    Json(req): Json<RepoPathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = req.repo_path;
    let result = spawn_blocking_handler!(move || service::get_repo_head(&repo_path))?;
    ok_json(result)
}

async fn get_session_token_usage(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_uuid = parse_uuid(&req.session_id)?;
    let app = state.app.clone();
    let data = spawn_blocking_handler!(move || {
        service::get_session_token_usage(&app, project_uuid, session_uuid)
    })?;
    ok_json(data)
}

// --- Directory Listing ---

async fn list_directories_at(
    Json(req): Json<PathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let entries =
        service::list_directories_at_safe(&req.path).map_err(<(StatusCode, String)>::from)?;
    ok_json(entries)
}

async fn list_root_directories(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let entries =
        service::list_root_directories(&state.app).map_err(<(StatusCode, String)>::from)?;
    ok_json(entries)
}

async fn generate_project_names(
    Json(req): Json<DescriptionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let description = req.description;
    let names = spawn_blocking_handler!(move || service::generate_project_names(&description))?;
    ok_json(names)
}

// --- Scaffold ---

async fn scaffold_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<NameRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project = service::scaffold_project(&state.app, &req.name)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(project)
}

// --- Session Management ---

async fn stage_session() -> Result<Json<Value>, (StatusCode, String)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "stage_session requires AppHandle and is not available in server mode".to_string(),
    ))
}

async fn unstage_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ProjectSessionRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_uuid = parse_uuid(&req.project_id)?;
    let session_uuid = parse_uuid(&req.session_id)?;

    service::unstage_session(&state.app, project_uuid, session_uuid)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn submit_secure_env_value(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<SubmitSecureEnvValueRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let status = service::submit_secure_env_value(&state.app, &req.request_id, &req.value)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(status)))
}

async fn cancel_secure_env_request(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<CancelSecureEnvRequestRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::cancel_secure_env_request(&state.app, &req.request_id)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

// --- Notes (additional) ---

async fn api_save_note_image(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<SaveNoteImageRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let filename =
        service::save_note_image(&state.app, &req.folder, &req.image_bytes, &req.extension)
            .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(filename)))
}

async fn api_resolve_note_asset_path(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<ResolveNoteAssetPathRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let resolved = service::resolve_note_asset_path(&state.app, &req.folder, &req.relative_path)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(resolved)))
}

async fn api_duplicate_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<FolderFilenameRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let copy = service::duplicate_note(&state.app, &req.folder, &req.filename)
        .map_err(<(StatusCode, String)>::from)?;
    ok_json(copy)
}

// --- Auth/Login ---

async fn start_claude_login(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id =
        service::start_claude_login(&state.app).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(session_id)))
}

async fn stop_claude_login(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(req): Json<SessionIdRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = parse_uuid(&req.session_id)?;
    service::stop_claude_login(&state.app, id).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
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
