use std::path::Path;

use tauri::State;
use uuid::Uuid;

use crate::models::Project;
use crate::state::AppState;

const DEFAULT_AGENTS_MD: &str = r#"# Agents

## Default Agent

You are a helpful coding assistant working on this project.
"#;

#[tauri::command]
pub fn create_project(
    state: State<AppState>,
    name: String,
    repo_path: String,
) -> Result<Project, String> {
    let path = Path::new(&repo_path);
    if !path.is_dir() {
        return Err(format!("repo_path is not a directory: {}", repo_path));
    }

    let project = Project {
        id: Uuid::new_v4(),
        name,
        repo_path: repo_path.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        archived: false,
        sessions: vec![],
    };

    let storage = state.storage.lock().map_err(|e| e.to_string())?;
    storage.save_project(&project).map_err(|e| e.to_string())?;

    // If repo doesn't have agents.md, create default one in config dir
    let repo_agents = path.join("agents.md");
    if !repo_agents.exists() {
        storage
            .save_agents_md(project.id, DEFAULT_AGENTS_MD)
            .map_err(|e| e.to_string())?;
    }

    Ok(project)
}

#[tauri::command]
pub fn load_project(
    state: State<AppState>,
    name: String,
    repo_path: String,
) -> Result<Project, String> {
    let path = Path::new(&repo_path);
    if !path.is_dir() {
        return Err(format!("repo_path is not a directory: {}", repo_path));
    }

    // Validate it's a git repo
    let git_dir = path.join(".git");
    if !git_dir.exists() {
        return Err(format!("not a git repository: {}", repo_path));
    }

    let project = Project {
        id: Uuid::new_v4(),
        name,
        repo_path: repo_path.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        archived: false,
        sessions: vec![],
    };

    let storage = state.storage.lock().map_err(|e| e.to_string())?;
    storage.save_project(&project).map_err(|e| e.to_string())?;

    // Only create default agents.md if repo doesn't have one
    let repo_agents = path.join("agents.md");
    if !repo_agents.exists() {
        storage
            .save_agents_md(project.id, DEFAULT_AGENTS_MD)
            .map_err(|e| e.to_string())?;
    }

    Ok(project)
}

#[tauri::command]
pub fn list_projects(state: State<AppState>) -> Result<Vec<Project>, String> {
    let storage = state.storage.lock().map_err(|e| e.to_string())?;
    let projects = storage.list_projects().map_err(|e| e.to_string())?;
    let active: Vec<Project> = projects.into_iter().filter(|p| !p.archived).collect();
    Ok(active)
}

#[tauri::command]
pub fn archive_project(state: State<AppState>, project_id: String) -> Result<(), String> {
    let id = Uuid::parse_str(&project_id).map_err(|e| e.to_string())?;
    let storage = state.storage.lock().map_err(|e| e.to_string())?;
    let mut project = storage.load_project(id).map_err(|e| e.to_string())?;
    project.archived = true;
    project.sessions.clear();
    storage.save_project(&project).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_agents_md(state: State<AppState>, project_id: String) -> Result<String, String> {
    let id = Uuid::parse_str(&project_id).map_err(|e| e.to_string())?;
    let storage = state.storage.lock().map_err(|e| e.to_string())?;
    let project = storage.load_project(id).map_err(|e| e.to_string())?;
    storage.get_agents_md(&project).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_agents_md(
    state: State<AppState>,
    project_id: String,
    content: String,
) -> Result<(), String> {
    let id = Uuid::parse_str(&project_id).map_err(|e| e.to_string())?;
    let storage = state.storage.lock().map_err(|e| e.to_string())?;
    storage
        .save_agents_md(id, &content)
        .map_err(|e| e.to_string())
}
