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
use serde_json::Value;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use the_controller_lib::{
    architecture, config, deploy, emitter::WsBroadcastEmitter, models, note_ai_chat, secure_env,
    service, state::AppState, status_socket,
};

use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

struct ServerState {
    app: Arc<AppState>,
    ws_tx: broadcast::Sender<String>,
}

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

// --- Route handlers ---

async fn list_projects(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let inventory = service::list_projects(&state.app).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(inventory).unwrap()))
}

async fn check_onboarding(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = service::check_onboarding(&state.app).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(cfg).unwrap()))
}

async fn restore_sessions(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    service::restore_sessions(&state.app.storage).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn connect_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let rows = args["rows"].as_u64().unwrap_or(24) as u16;
    let cols = args["cols"].as_u64().unwrap_or(80) as u16;
    let id =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    service::connect_session(&state.app, id, rows, cols).map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn load_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing name".to_string()))?;
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;

    let project =
        service::load_project(&state.app, name, repo_path).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(project).unwrap()))
}

async fn write_to_pty(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let data = args["data"].as_str().unwrap_or_default();
    let id =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::write_to_pty(&state.app, id, data.as_bytes()).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn send_raw_to_pty(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let data = args["data"].as_str().unwrap_or_default();
    let id =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::send_raw_to_pty(&state.app, id, data.as_bytes())
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn resize_pty(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let rows = args["rows"].as_u64().unwrap_or(24) as u16;
    let cols = args["cols"].as_u64().unwrap_or(80) as u16;
    let id =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::resize_pty(&state.app, id, rows, cols).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn close_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let delete_worktree = args["deleteWorktree"].as_bool().unwrap_or(false);
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    service::close_session(&state.app, project_uuid, session_uuid, delete_worktree)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn create_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let kind = args["kind"].as_str().unwrap_or("claude").to_string();
    let background = args["background"].as_bool().unwrap_or(false);
    let initial_prompt = args["initialPrompt"].as_str().map(|s| s.to_string());
    let github_issue: Option<models::GithubIssue> =
        serde_json::from_value(args["githubIssue"].clone()).ok();

    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_id = uuid::Uuid::new_v4();

    let storage = state.app.storage.clone();
    let pty_manager = state.app.pty_manager.clone();
    let emitter = state.app.emitter.clone();

    let result = tokio::task::spawn_blocking(move || {
        service::create_session(
            &storage,
            &pty_manager,
            &emitter,
            project_uuid,
            session_id,
            &kind,
            github_issue,
            background,
            initial_prompt,
        )
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?
    .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::String(result)))
}

async fn generate_architecture(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?
        .to_string();
    let emitter = state.app.emitter.clone();
    let result = tokio::task::spawn_blocking(move || {
        architecture::generate_architecture_blocking_with_emitter(
            std::path::Path::new(&repo_path),
            &emitter,
        )
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::to_value(result).unwrap()))
}

async fn create_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing name".to_string()))?;
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;

    let project = service::create_project(&state.app, name, repo_path)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(project).unwrap()))
}

async fn delete_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let delete_repo = args["deleteRepo"].as_bool().unwrap_or(false);
    let id =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    service::delete_project(&state.app.storage, &state.app.pty_manager, id, delete_repo)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn get_agents_md(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let id =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let content = service::get_agents_md(&state.app, id).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(content)))
}

async fn update_agents_md(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let content = args["content"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing content".to_string()))?;
    let id =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    service::update_agents_md(&state.app, id, content).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn set_initial_prompt(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let prompt = args["prompt"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing prompt".to_string()))?
        .to_string();

    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    service::set_initial_prompt(&state.app, project_uuid, session_uuid, prompt)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn check_claude_cli() -> Result<Json<Value>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(service::check_claude_cli)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task failed: {}", e),
            )
        })?;
    Ok(Json(Value::String(result)))
}

async fn home_dir() -> Result<Json<Value>, (StatusCode, String)> {
    let path = service::home_dir().map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(path)))
}

async fn save_onboarding_config(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let projects_root = args["projectsRoot"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectsRoot".to_string()))?
        .to_string();
    let default_provider: config::ConfigDefaultProvider =
        serde_json::from_value(args["defaultProvider"].clone()).unwrap_or_default();

    service::save_onboarding_config(&state.app, &projects_root, Some(default_provider))
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn log_frontend_error(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let message = args["message"].as_str().unwrap_or_default();
    service::log_frontend_error(&state.app, message);
    Ok(Json(Value::Null))
}

async fn detect_project_type(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?
        .to_string();
    let result =
        tokio::task::spawn_blocking(move || service::detect_project_type_blocking(&repo_path))
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

async fn get_deploy_credentials() -> Result<Json<Value>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(service::get_deploy_credentials_blocking)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

async fn save_deploy_credentials(
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let credentials: deploy::credentials::DeployCredentials =
        serde_json::from_value(args["credentials"].clone())
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    tokio::task::spawn_blocking(move || service::save_deploy_credentials_blocking(credentials))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn is_deploy_provisioned() -> Result<Json<Value>, (StatusCode, String)> {
    let provisioned = tokio::task::spawn_blocking(service::is_deploy_provisioned_blocking)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Bool(provisioned)))
}

async fn deploy_project(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let request: deploy::commands::DeployRequest = serde_json::from_value(args["request"].clone())
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let result = service::deploy_project(request)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(result).unwrap()))
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
    Ok(Json(serde_json::to_value(result).unwrap()))
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
    let storage = state.app.storage.clone();
    let theme = tokio::task::spawn_blocking(move || {
        let base_dir = storage.lock().map_err(|e| e.to_string())?;
        let base_dir = base_dir.base_dir();
        the_controller_lib::terminal_theme::load_terminal_theme(&base_dir)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::to_value(theme).unwrap()))
}

async fn list_archived_projects(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let filtered =
        service::list_archived_projects(&state.app).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(filtered).unwrap()))
}
async fn merge_session_branch(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    const OVERALL_TIMEOUT_SECS: u64 = 600; // 10 minutes

    let merge_result = tokio::time::timeout(
        std::time::Duration::from_secs(OVERALL_TIMEOUT_SECS),
        service::merge_session_branch(&state.app, project_uuid, session_uuid, true),
    )
    .await;

    match merge_result {
        Ok(inner) => {
            let resp = inner.map_err(<(StatusCode, String)>::from)?;
            Ok(Json(serde_json::to_value(resp).unwrap()))
        }
        Err(_) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            format!("Merge timed out after {} seconds", OVERALL_TIMEOUT_SECS),
        )),
    }
}

async fn send_note_ai_chat(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let note_content = args["noteContent"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing noteContent".to_string()))?
        .to_string();
    let selected_text = args["selectedText"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing selectedText".to_string()))?
        .to_string();
    let prompt = args["prompt"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing prompt".to_string()))?
        .to_string();

    let conversation_history: Vec<note_ai_chat::NoteAiChatMessage> =
        serde_json::from_value(args["conversationHistory"].clone()).unwrap_or_default();

    let response =
        service::send_note_ai_chat(note_content, selected_text, conversation_history, prompt)
            .await
            .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(serde_json::to_value(response).unwrap()))
}
// --- Notes ---

async fn api_list_notes(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let entries =
        service::list_notes(&state.app.storage, &folder).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(entries).unwrap()))
}

async fn api_read_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let filename = args["filename"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing filename".to_string()))?
        .to_string();
    let content = service::read_note(&state.app.storage, &folder, &filename)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(content).unwrap()))
}

async fn api_write_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let filename = args["filename"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing filename".to_string()))?
        .to_string();
    let content = args["content"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing content".to_string()))?
        .to_string();
    service::write_note(&state.app.storage, &folder, &filename, &content)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_create_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let title = args["title"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing title".to_string()))?
        .to_string();
    let filename = service::create_note(&state.app.storage, &folder, &title)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(filename).unwrap()))
}

async fn api_delete_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let filename = args["filename"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing filename".to_string()))?
        .to_string();
    service::delete_note(&state.app.storage, &folder, &filename)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_rename_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let old_name = args["oldName"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing oldName".to_string()))?
        .to_string();
    let new_name = args["newName"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing newName".to_string()))?
        .to_string();
    let filename = service::rename_note(&state.app.storage, &folder, &old_name, &new_name)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(filename).unwrap()))
}

async fn api_list_folders(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(_args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folders =
        service::list_note_folders(&state.app.storage).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(folders).unwrap()))
}

async fn api_create_folder(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing name".to_string()))?
        .to_string();
    service::create_note_folder(&state.app.storage, &name).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_rename_folder(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let old_name = args["oldName"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing oldName".to_string()))?
        .to_string();
    let new_name = args["newName"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing newName".to_string()))?
        .to_string();
    service::rename_note_folder(&state.app.storage, &old_name, &new_name)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_delete_folder(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing name".to_string()))?
        .to_string();
    let force = args["force"].as_bool().unwrap_or(false);
    service::delete_note_folder(&state.app.storage, &name, force)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn api_commit_notes(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let committed =
        service::commit_pending_notes(&state.app.storage).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(committed).unwrap()))
}

// --- GitHub Issues ---

async fn list_github_issues(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;

    let issues = service::list_github_issues(&state.app, repo_path)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(issues).unwrap()))
}

async fn list_assigned_issues(
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;

    let assigned = service::list_assigned_issues(repo_path)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(assigned).unwrap()))
}

async fn create_github_issue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;
    let title = args["title"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing title".to_string()))?;
    let body = args["body"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing body".to_string()))?;

    let issue = service::create_github_issue(&state.app, repo_path, title, body)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(issue).unwrap()))
}

async fn generate_issue_body(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let title = args["title"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing title".to_string()))?;

    let body = service::generate_issue_body(title)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(body)))
}

async fn post_github_comment(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;
    let issue_number = args["issueNumber"]
        .as_u64()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing issueNumber".to_string()))?;
    let body = args["body"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing body".to_string()))?;

    service::post_github_comment(repo_path, issue_number, body)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn add_github_label(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;
    let issue_number = args["issueNumber"]
        .as_u64()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing issueNumber".to_string()))?;
    let label = args["label"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing label".to_string()))?;
    let description = args["description"].as_str();
    let color = args["color"].as_str();

    service::add_github_label(
        &state.app,
        repo_path,
        issue_number,
        label,
        description,
        color,
    )
    .await
    .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn remove_github_label(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;
    let issue_number = args["issueNumber"]
        .as_u64()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing issueNumber".to_string()))?;
    let label = args["label"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing label".to_string()))?;

    service::remove_github_label(&state.app, repo_path, issue_number, label)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn close_github_issue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;
    let issue_number = args["issueNumber"]
        .as_u64()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing issueNumber".to_string()))?;
    let comment = args["comment"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing comment".to_string()))?;

    service::close_github_issue(&state.app, repo_path, issue_number, comment)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn delete_github_issue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;
    let issue_number = args["issueNumber"]
        .as_u64()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing issueNumber".to_string()))?;

    service::delete_github_issue(&state.app, repo_path, issue_number)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

// --- Maintainer & Auto-Worker ---

async fn configure_maintainer(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let enabled = args["enabled"].as_bool().unwrap_or(false);
    let interval_minutes = args["intervalMinutes"].as_u64().unwrap_or(30);
    let github_repo = args["githubRepo"].as_str().map(|s| s.to_string());
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::configure_maintainer(
        &state.app,
        project_uuid,
        enabled,
        interval_minutes,
        github_repo,
    )
    .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn get_maintainer_status(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let log = service::get_maintainer_status(&state.app, project_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(log).unwrap()))
}

async fn get_maintainer_history(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let logs = service::get_maintainer_history(&state.app, project_uuid, 20)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(logs).unwrap()))
}

async fn trigger_maintainer_check(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let log = service::trigger_maintainer_check(&state.app, project_uuid)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(log).unwrap()))
}

async fn clear_maintainer_reports(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::clear_maintainer_reports(&state.app, project_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn get_maintainer_issues(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let (repo_path, github_repo) = {
        let storage = state
            .app
            .storage
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let project = storage
            .load_project(project_uuid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        (
            project.repo_path.clone(),
            project.maintainer.github_repo.clone(),
        )
    };

    let issues = service::get_maintainer_issues(&repo_path, github_repo.as_deref())
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(issues).unwrap()))
}

async fn get_maintainer_issue_detail(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let issue_number = args["issueNumber"]
        .as_u64()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing issueNumber".to_string()))?
        as u32;
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let (repo_path, github_repo) = {
        let storage = state
            .app
            .storage
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let project = storage
            .load_project(project_uuid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        (
            project.repo_path.clone(),
            project.maintainer.github_repo.clone(),
        )
    };

    let detail =
        service::get_maintainer_issue_detail(&repo_path, github_repo.as_deref(), issue_number)
            .await
            .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(detail).unwrap()))
}

async fn configure_auto_worker(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let enabled = args["enabled"].as_bool().unwrap_or(false);
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::configure_auto_worker(&state.app, project_uuid, enabled)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn get_auto_worker_queue(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing projectId".to_string()))?;
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let queue = service::get_auto_worker_queue(&state.app, project_uuid)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(queue).unwrap()))
}

async fn get_worker_reports(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?;

    let reports = service::get_worker_reports(repo_path)
        .await
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(reports).unwrap()))
}

// --- Storage/Git Operations ---

async fn get_session_commits(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let storage = state.app.storage.clone();
    let commits = tokio::task::spawn_blocking(move || {
        service::get_session_commits(&storage, project_uuid, session_uuid)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?
    .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(commits).unwrap()))
}

async fn save_session_prompt(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::save_session_prompt(&state.app, project_uuid, session_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::Null))
}

async fn list_project_prompts(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let prompts = service::list_project_prompts(&state.app, project_uuid)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(prompts).unwrap()))
}

async fn get_repo_head(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?
        .to_string();
    let result = tokio::task::spawn_blocking(move || service::get_repo_head(&repo_path))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task failed: {}", e),
            )
        })?
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(result).unwrap()))
}

async fn get_session_token_usage(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let storage = state.app.storage.clone();
    let data = tokio::task::spawn_blocking(move || {
        service::get_session_token_usage(&storage, project_uuid, session_uuid)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?
    .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(data).unwrap()))
}

// --- Directory Listing ---

async fn list_directories_at(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let path = args["path"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing path".to_string()))?
        .to_string();
    // Restrict to directories under $HOME to prevent arbitrary filesystem enumeration
    let p = Path::new(&path);
    let requested = std::fs::canonicalize(p).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("cannot resolve path: {}", e),
        )
    })?;
    let home = dirs::home_dir().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot determine home directory".to_string(),
        )
    })?;
    if !requested.starts_with(&home) {
        return Err((
            StatusCode::FORBIDDEN,
            "path must be under the home directory".to_string(),
        ));
    }
    let entries = service::list_directories_at(&path).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(entries).unwrap()))
}

async fn list_root_directories(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let entries =
        service::list_root_directories(&state.app).map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(entries).unwrap()))
}

async fn generate_project_names(
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let description = args["description"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing description".to_string()))?
        .to_string();
    let names = tokio::task::spawn_blocking(move || service::generate_project_names(&description))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task failed: {}", e),
            )
        })?
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(names).unwrap()))
}

// --- Scaffold ---

async fn scaffold_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing name".to_string()))?
        .to_string();
    service::validate_project_name(&name).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let repo_path = {
        let storage = state
            .app
            .storage
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if let Ok(inventory) = storage.list_projects() {
            if inventory.projects.iter().any(|p| p.name == name) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("A project named '{}' already exists", name),
                ));
            }
        }
        let cfg = config::load_config(&storage.base_dir()).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "No config found. Complete onboarding first.".to_string(),
            )
        })?;
        PathBuf::from(&cfg.projects_root).join(&name)
    };
    if repo_path.exists() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Directory already exists: {}", name),
        ));
    }
    let name_clone = name.clone();
    let project = tokio::task::spawn_blocking(move || {
        service::scaffold_project_blocking(name_clone, repo_path)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if let Ok(inventory) = storage.list_projects() {
        if inventory.projects.iter().any(|p| p.name == project.name) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("A project named '{}' already exists", project.name),
            ));
        }
    }
    storage
        .save_project(&project)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::to_value(project).unwrap()))
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
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_id_str = args["sessionId"].as_str().unwrap_or_default();
    let session_uuid = uuid::Uuid::parse_str(session_id_str)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    service::unstage_session(&state.app, project_uuid, session_uuid)
        .map_err(<(StatusCode, String)>::from)?;

    Ok(Json(Value::Null))
}

async fn submit_secure_env_value(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let request_id = args["requestId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing requestId".to_string()))?
        .to_string();
    let value = args["value"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing value".to_string()))?
        .to_string();
    let (pending, response_tx) = secure_env::take_secure_env_submission(&state.app, &request_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let req_id = request_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        secure_env::update_env_file(&pending.env_path, &pending.key, &value)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {e}"),
        )
    })?;
    let result = secure_env::finish_secure_env_submission(&req_id, response_tx, result)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let status = if result.created { "created" } else { "updated" };
    Ok(Json(Value::String(status.to_string())))
}

async fn cancel_secure_env_request(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let request_id = args["requestId"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing requestId".to_string()))?
        .to_string();
    secure_env::cancel_secure_env_request(&state.app, &request_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(Value::Null))
}

// --- Notes (additional) ---

async fn api_save_note_image(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let extension = args["extension"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing extension".to_string()))?
        .to_string();
    let image_bytes: Vec<u8> = serde_json::from_value(args["imageBytes"].clone()).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid imageBytes: {}", e),
        )
    })?;
    let filename = service::save_note_image(&state.app.storage, &folder, &image_bytes, &extension)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(filename)))
}

async fn api_resolve_note_asset_path(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let relative_path = args["relativePath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing relativePath".to_string()))?
        .to_string();
    let resolved = service::resolve_note_asset_path(&state.app.storage, &folder, &relative_path)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(resolved)))
}

async fn api_duplicate_note(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let filename = args["filename"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing filename".to_string()))?
        .to_string();
    let copy = service::duplicate_note(&state.app.storage, &folder, &filename)
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(serde_json::to_value(copy).unwrap()))
}

// --- Auth/Login ---

async fn start_claude_login(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id = service::start_claude_login(&state.app.pty_manager, state.app.emitter.clone())
        .map_err(<(StatusCode, String)>::from)?;
    Ok(Json(Value::String(session_id)))
}

async fn stop_claude_login(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let id =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    service::stop_claude_login(&state.app.pty_manager, id).map_err(<(StatusCode, String)>::from)?;
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
    use crate::{auth_middleware, handle_ws, startup_messages, ServerState};
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
}
