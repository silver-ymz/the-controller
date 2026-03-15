use axum::{
    extract::{
        ws::{Message, WebSocket},
        Request, State as AxumState, WebSocketUpgrade,
    },
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use the_controller_lib::{
    architecture, commands, config, emitter::WsBroadcastEmitter, models, note_ai_chat, notes,
    session_args, state::AppState, status_socket, worktree::WorktreeManager,
};

use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

struct ServerState {
    app: Arc<AppState>,
    ws_tx: broadcast::Sender<String>,
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

    println!("\nShutting down...");
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
    let (emitter, ws_tx) = WsBroadcastEmitter::new();
    let app_state = Arc::new(AppState::new(emitter).expect("Failed to initialize app state"));

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

    let serve_dir = ServeDir::new(&dist_dir)
        .not_found_service(ServeFile::new(format!("{}/index.html", dist_dir)));

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
        .route(
            "/api/copy_image_file_to_clipboard",
            post(copy_image_file_to_clipboard),
        )
        .route("/api/capture_app_screenshot", post(capture_app_screenshot))
        .route("/api/start_voice_pipeline", post(start_voice_pipeline))
        .route("/api/stop_voice_pipeline", post(stop_voice_pipeline))
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
        .route("/ws", get(ws_upgrade))
        .fallback_service(serve_dir)
        .layer(middleware::from_fn(auth_middleware))
        .layer(CorsLayer::permissive())
        .with_state(state.clone());

    let port = get_port();
    let bind = get_bind_address();
    let addr = format!("{}:{}", bind, port);
    let token = std::env::var("CONTROLLER_AUTH_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    match &token {
        Some(t) => println!("Server listening on http://{}?token={}", addr, t),
        None => println!("Server listening on http://{} (no auth)", addr),
    }
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await
        .unwrap();
}

// --- Auth middleware ---

async fn auth_middleware(req: Request, next: Next) -> Result<Response, StatusCode> {
    let token = match std::env::var("CONTROLLER_AUTH_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => return Ok(next.run(req).await), // No token configured = no auth
    };

    // Skip auth for static files (non-API, non-WS)
    let path = req.uri().path();
    if !path.starts_with("/api/") && path != "/ws" {
        return Ok(next.run(req).await);
    }

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
            .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("token=")))
            .map(|t| t == token)
            .unwrap_or(false);

    if authorized {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

// --- Route handlers ---

async fn list_projects(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let inventory = storage
        .list_projects()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::to_value(inventory).unwrap()))
}

async fn check_onboarding(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let base_dir = storage.base_dir();
    let cfg = config::load_config(&base_dir);
    Ok(Json(serde_json::to_value(cfg).unwrap()))
}

async fn restore_sessions(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let inventory = storage
        .list_projects()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    inventory.warn_if_corrupt("restore_sessions");
    for project in &inventory.projects {
        if let Err(e) = storage.migrate_worktree_paths(project) {
            eprintln!(
                "Failed to migrate worktrees for project '{}': {}",
                project.name, e
            );
        }
    }
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

    // Check if already connected
    {
        let pty_manager = state
            .app
            .pty_manager
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if pty_manager.session_ids().contains(&id) {
            return Ok(Json(Value::Null));
        }
    }

    // Find session config from storage
    let (session_dir, kind) = {
        let storage = state
            .app
            .storage
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let inventory = storage
            .list_projects()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        inventory.warn_if_corrupt("connect_session");
        inventory
            .projects
            .iter()
            .flat_map(|p| p.sessions.iter().map(move |s| (p, s)))
            .find(|(_, s)| s.id == id)
            .map(|(p, s)| {
                let dir = s
                    .worktree_path
                    .clone()
                    .unwrap_or_else(|| p.repo_path.clone());
                (dir, s.kind.clone())
            })
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("session not found: {}", session_id),
                )
            })?
    };

    let pty_manager = state.app.pty_manager.clone();
    let emitter = state.app.emitter.clone();
    tokio::task::spawn_blocking(move || {
        let mut mgr = pty_manager
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        mgr.spawn_session(id, &session_dir, &kind, emitter, true, None, rows, cols)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })??;

    Ok(Json(Value::Null))
}

async fn load_project(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let id =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let project = storage
        .load_project(id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
    let mut pty = state
        .app
        .pty_manager
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    pty.write_to_session(id, data.as_bytes())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
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
    let mut pty = state
        .app
        .pty_manager
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    pty.send_raw_to_session(id, data.as_bytes())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
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
    let pty = state
        .app
        .pty_manager
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    pty.resize_session(id, rows, cols)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(Value::Null))
}

async fn close_session(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let id =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let mut pty = state
        .app
        .pty_manager
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let _ = pty.close_session(id);
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

    // Load the project and generate session label
    let (repo_path, label, base_dir, project_name) = {
        let storage = state
            .app
            .storage
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let project = storage
            .load_project(project_uuid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let label = commands::next_session_label(&project.sessions);
        (
            project.repo_path.clone(),
            label,
            storage.base_dir(),
            project.name.clone(),
        )
    };

    // Create worktree under ~/.the-controller/worktrees/{project_name}/{label}/
    let worktree_dir = base_dir.join("worktrees").join(&project_name).join(&label);

    let repo_path_clone = repo_path.clone();
    let label_clone = label.clone();
    let (session_dir, wt_path, wt_branch) = tokio::task::spawn_blocking(move || {
        match WorktreeManager::create_worktree(&repo_path_clone, &label_clone, &worktree_dir) {
            Ok(worktree_path) => {
                let wt_str = worktree_path
                    .to_str()
                    .ok_or_else(|| "worktree path is not valid UTF-8".to_string())?
                    .to_string();
                Ok((wt_str.clone(), Some(wt_str), Some(label_clone)))
            }
            Err(e) if e == "unborn_branch" => Ok((repo_path_clone, None, None)),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Build initial prompt: explicit prompt takes priority, then GitHub issue context
    let initial_prompt = initial_prompt.or_else(|| {
        github_issue.as_ref().map(|issue| {
            session_args::build_issue_prompt(issue.number, &issue.title, &issue.url, background)
        })
    });

    let session_config = models::SessionConfig {
        id: session_id,
        label: label.clone(),
        worktree_path: wt_path.clone(),
        worktree_branch: wt_branch.clone(),
        archived: false,
        kind: kind.clone(),
        github_issue,
        initial_prompt: initial_prompt.clone(),
        done_commits: vec![],
        auto_worker_session: false,
    };

    // Save session config to project
    {
        let storage = state
            .app
            .storage
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let mut project = storage
            .load_project(project_uuid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        project.sessions.push(session_config);
        storage
            .save_project(&project)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    // Spawn PTY session
    let pty_manager = state.app.pty_manager.clone();
    let emitter = state.app.emitter.clone();
    let spawn_result = tokio::task::spawn_blocking(move || {
        let mut mgr = pty_manager.lock().map_err(|e| e.to_string())?;
        mgr.spawn_session(
            session_id,
            &session_dir,
            &kind,
            emitter,
            false,
            initial_prompt.as_deref(),
            24,
            80,
        )
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task failed: {}", e),
        )
    })?;

    if let Err(spawn_err) = spawn_result {
        // Rollback: remove session from project
        if let Ok(storage) = state.app.storage.lock() {
            if let Ok(mut project) = storage.load_project(project_uuid) {
                project.sessions.retain(|s| s.id != session_id);
                let _ = storage.save_project(&project);
            }
        }
        // Cleanup worktree
        if let (Some(ref wt_path), Some(ref wt_branch)) = (wt_path, wt_branch) {
            let _ = WorktreeManager::remove_worktree(wt_path, &repo_path, wt_branch);
        }
        return Err((StatusCode::INTERNAL_SERVER_ERROR, spawn_err));
    }

    Ok(Json(Value::String(session_id.to_string())))
}

async fn generate_architecture(
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?
        .to_string();

    let result = tokio::task::spawn_blocking(move || {
        architecture::generate_architecture_blocking(std::path::Path::new(&repo_path))
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
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing name".to_string()))?
        .to_string();
    let repo_path = args["repoPath"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing repoPath".to_string()))?
        .to_string();

    commands::validate_project_name(&name).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let path = std::path::Path::new(&repo_path);
    if !path.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("repo_path is not a directory: {}", repo_path),
        ));
    }

    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Reject duplicate project names
    if let Ok(inventory) = storage.list_projects() {
        if inventory.projects.iter().any(|p| p.name == name) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("A project named '{}' already exists", name),
            ));
        }
    }

    let project = models::Project {
        id: uuid::Uuid::new_v4(),
        name: name.clone(),
        repo_path: repo_path.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        archived: false,
        maintainer: models::MaintainerConfig::default(),
        auto_worker: models::AutoWorkerConfig::default(),
        prompts: vec![],
        sessions: vec![],
        staged_session: None,
    };

    storage
        .save_project(&project)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // If repo doesn't have agents.md, create default one in config dir
    let repo_agents = path.join("agents.md");
    if !repo_agents.exists() {
        storage
            .save_agents_md(project.id, &commands::render_agents_md(&project.name))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    // If repo has agents.md but no CLAUDE.md, create symlink
    commands::ensure_claude_md_symlink(path).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

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

    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let project = storage
        .load_project(id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Close all PTY sessions and clean up worktrees
    {
        let mut pty_manager = state
            .app
            .pty_manager
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        for session in &project.sessions {
            let _ = pty_manager.close_session(session.id);
            if let (Some(wt_path), Some(branch)) =
                (&session.worktree_path, &session.worktree_branch)
            {
                let _ = WorktreeManager::remove_worktree(wt_path, &project.repo_path, branch);
            }
        }
    }

    // Delete project metadata
    storage
        .delete_project_dir(id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Optionally delete the repo directory
    if delete_repo && std::path::Path::new(&project.repo_path).exists() {
        std::fs::remove_dir_all(&project.repo_path).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to delete repo: {}", e),
            )
        })?;
    }

    Ok(Json(Value::Null))
}

async fn get_agents_md(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let project_id = args["projectId"].as_str().unwrap_or_default();
    let id =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let project = storage
        .load_project(id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let content = storage
        .get_agents_md(&project)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    storage
        .save_agents_md(id, content)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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

    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut project = storage
        .load_project(project_uuid)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(session) = project.sessions.iter_mut().find(|s| s.id == session_uuid) {
        if session.initial_prompt.is_none() {
            session.initial_prompt = Some(prompt);
            storage
                .save_project(&project)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
    }

    Ok(Json(Value::Null))
}

async fn check_claude_cli() -> Result<Json<Value>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(config::check_claude_cli_status)
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
    let path = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not determine home directory".to_string(),
            )
        })?;
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

    let path = std::path::Path::new(&projects_root);
    if !path.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "projects_root is not an existing directory: {}",
                projects_root
            ),
        ));
    }

    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let base_dir = storage.base_dir();
    let cfg = config::Config {
        projects_root,
        default_provider,
    };
    config::save_config(&base_dir, &cfg)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(Value::Null))
}

async fn log_frontend_error(Json(args): Json<Value>) -> Result<Json<Value>, (StatusCode, String)> {
    let message = args["message"].as_str().unwrap_or_default();
    eprintln!("[FRONTEND] {}", message);
    Ok(Json(Value::Null))
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

async fn start_voice_pipeline() -> Result<Json<Value>, (StatusCode, String)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "start_voice_pipeline is not available in server mode".to_string(),
    ))
}

async fn stop_voice_pipeline() -> Result<Json<Value>, (StatusCode, String)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "stop_voice_pipeline is not available in server mode".to_string(),
    ))
}

async fn load_terminal_theme() -> Result<Json<Value>, (StatusCode, String)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        "load_terminal_theme is not available in server mode".to_string(),
    ))
}

async fn list_archived_projects(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let storage = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let inventory = storage
        .list_projects()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let filtered = inventory.filter_projects(|project| {
        project.archived || project.sessions.iter().any(|session| session.archived)
    });
    Ok(Json(serde_json::to_value(filtered).unwrap()))
}
async fn merge_session_branch(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    use the_controller_lib::models::MergeResponse;
    use the_controller_lib::worktree::{MergeResult, WorktreeManager};

    let project_id = args["projectId"].as_str().unwrap_or_default();
    let session_id = args["sessionId"].as_str().unwrap_or_default();
    let project_uuid =
        uuid::Uuid::parse_str(project_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let session_uuid =
        uuid::Uuid::parse_str(session_id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let (repo_path, worktree_path, branch_name) = {
        let storage = state
            .app
            .storage
            .lock()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let project = storage
            .load_project(project_uuid)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let session = project
            .sessions
            .iter()
            .find(|s| s.id == session_uuid)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "Session not found".to_string()))?;
        let wt_path = session.worktree_path.clone().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "Session has no worktree".to_string(),
            )
        })?;
        let branch = session
            .worktree_branch
            .clone()
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "Session has no branch".to_string()))?;
        (project.repo_path.clone(), wt_path, branch)
    };

    const MAX_RETRIES: u32 = 5;
    const POLL_INTERVAL_SECS: u64 = 3;

    for attempt in 0..MAX_RETRIES {
        let rp = repo_path.clone();
        let wt = worktree_path.clone();
        let br = branch_name.clone();

        let result = tokio::task::spawn_blocking(move || {
            if WorktreeManager::is_rebase_in_progress(&wt) {
                Ok(MergeResult::RebaseConflicts)
            } else {
                WorktreeManager::merge_via_pr(&rp, &wt, &br)
            }
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task failed: {}", e),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        match result {
            MergeResult::PrCreated(url) => {
                let resp = MergeResponse::PrCreated { url };
                return Ok(Json(serde_json::to_value(resp).unwrap()));
            }
            MergeResult::RebaseConflicts => {
                let prompt = "merge\r";
                {
                    let mut pty_manager = state
                        .app
                        .pty_manager
                        .lock()
                        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                    let _ = pty_manager.write_to_session(session_uuid, prompt.as_bytes());
                }

                let _ = state.app.emitter.emit(
                    "merge-status",
                    &format!(
                        "Rebase conflicts (attempt {}/{}). Claude is resolving...",
                        attempt + 1,
                        MAX_RETRIES
                    ),
                );

                let wt_poll = worktree_path.clone();
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(POLL_INTERVAL_SECS)).await;
                    let wt_check = wt_poll.clone();
                    let still_rebasing = tokio::task::spawn_blocking(move || {
                        WorktreeManager::is_rebase_in_progress(&wt_check)
                    })
                    .await
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Task failed: {}", e),
                        )
                    })?;
                    if !still_rebasing {
                        break;
                    }
                }
                continue;
            }
        }
    }

    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        format!(
            "Merge failed after {} attempts due to recurring conflicts",
            MAX_RETRIES
        ),
    ))
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

    let response = note_ai_chat::send_note_ai_message(
        std::env::temp_dir().to_string_lossy().to_string(),
        note_content,
        selected_text,
        conversation_history,
        prompt,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::to_value(response).unwrap()))
}
// --- Notes ---

fn server_try_commit(state: &Arc<ServerState>, message: &str) {
    if let Ok(storage) = state.app.storage.lock() {
        let base_dir = storage.base_dir();
        if let Err(e) = notes::commit_notes(&base_dir, message) {
            eprintln!("notes git commit failed: {}", e);
        }
    }
}

async fn api_list_notes(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let folder = args["folder"]
        .as_str()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "missing folder".to_string()))?
        .to_string();
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    let entries = notes::list_notes(&base_dir, &folder)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    let content = notes::read_note(&base_dir, &folder, &filename)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    notes::write_note(&base_dir, &folder, &filename, &content)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    let filename = notes::create_note(&base_dir, &folder, &title)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    server_try_commit(&state, &format!("create {}/{}", folder, filename));
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    notes::delete_note(&base_dir, &folder, &filename)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    server_try_commit(&state, &format!("delete {}/{}", folder, filename));
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    let filename = notes::rename_note(&base_dir, &folder, &old_name, &new_name)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    server_try_commit(
        &state,
        &format!("rename {}/{} → {}", folder, old_name, filename),
    );
    Ok(Json(serde_json::to_value(filename).unwrap()))
}

async fn api_list_folders(
    AxumState(state): AxumState<Arc<ServerState>>,
    Json(_args): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    let folders = notes::list_folders(&base_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    notes::create_folder(&base_dir, &name)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    server_try_commit(&state, &format!("create folder {}", name));
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    notes::rename_folder(&base_dir, &old_name, &new_name)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    server_try_commit(
        &state,
        &format!("rename folder {} → {}", old_name, new_name),
    );
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
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    notes::delete_folder(&base_dir, &name, force)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    server_try_commit(&state, &format!("delete folder {}", name));
    Ok(Json(Value::Null))
}

async fn api_commit_notes(
    AxumState(state): AxumState<Arc<ServerState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let base_dir = state
        .app
        .storage
        .lock()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .base_dir();
    let committed = notes::commit_notes(&base_dir, "update notes")
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::to_value(committed).unwrap()))
}

// --- WebSocket ---

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    AxumState(state): AxumState<Arc<ServerState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state.ws_tx.subscribe()))
}

async fn handle_ws(mut socket: WebSocket, mut rx: broadcast::Receiver<String>) {
    while let Ok(msg) = rx.recv().await {
        if socket.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}
