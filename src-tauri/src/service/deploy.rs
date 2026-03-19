use std::path::Path;

use crate::deploy::commands::{DeployRequest, DeployResult, ProjectSignals};
use crate::deploy::coolify::CoolifyClient;
use crate::deploy::credentials::DeployCredentials;
use crate::error::AppError;
use the_controller_macros::derive_handlers;

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn detect_project_type_blocking(repo_path: &str) -> Result<ProjectSignals, AppError> {
    tracing::debug!(repo_path = %repo_path, "detecting project type");
    let path = Path::new(repo_path);
    let has_package_json = path.join("package.json").exists();
    let has_start_script = if has_package_json {
        std::fs::read_to_string(path.join("package.json"))
            .map(|content| content.contains("\"start\""))
            .unwrap_or(false)
    } else {
        false
    };

    Ok(ProjectSignals {
        has_dockerfile: path.join("Dockerfile").exists(),
        has_package_json,
        has_vite_config: path.join("vite.config.ts").exists()
            || path.join("vite.config.js").exists()
            || path.join("astro.config.mjs").exists()
            || path.join("next.config.js").exists()
            || path.join("next.config.mjs").exists(),
        has_start_script,
        has_pyproject: path.join("pyproject.toml").exists()
            || path.join("requirements.txt").exists(),
    })
}

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn get_deploy_credentials_blocking() -> Result<DeployCredentials, AppError> {
    tracing::debug!("loading deploy credentials");
    DeployCredentials::load().map_err(AppError::Internal)
}

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn save_deploy_credentials_blocking(credentials: DeployCredentials) -> Result<(), AppError> {
    tracing::info!("saving deploy credentials");
    credentials.save().map_err(AppError::Internal)
}

#[derive_handlers(tauri_command, axum_handler, blocking)]
pub fn is_deploy_provisioned_blocking() -> Result<bool, AppError> {
    let creds = DeployCredentials::load().map_err(AppError::Internal)?;
    Ok(creds.is_provisioned())
}

#[derive_handlers(tauri_command, axum_handler)]
pub async fn deploy_project(request: DeployRequest) -> Result<DeployResult, AppError> {
    tracing::info!(
        project = %request.project_name,
        subdomain = %request.subdomain,
        project_type = %request.project_type,
        "starting project deployment"
    );
    let creds = tokio::task::spawn_blocking(DeployCredentials::load)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(AppError::Internal)?;
    if !creds.is_provisioned() {
        tracing::error!("deploy not provisioned — credentials incomplete");
        return Err(AppError::BadRequest(
            "Deploy not provisioned. Run setup first.".to_string(),
        ));
    }

    let coolify = CoolifyClient::new(
        creds
            .coolify_url
            .as_ref()
            .ok_or_else(|| AppError::Internal("Coolify URL not configured".to_string()))?,
        creds
            .coolify_api_key
            .as_ref()
            .ok_or_else(|| AppError::Internal("Coolify API key not configured".to_string()))?,
    );

    let apps = coolify
        .list_applications()
        .await
        .map_err(AppError::Internal)?;
    let existing = apps.iter().find(|a| a.name == request.project_name);

    let uuid = if let Some(app) = existing {
        tracing::info!(uuid = %app.uuid, "found existing Coolify app, redeploying");
        coolify
            .deploy_application(&app.uuid)
            .await
            .map_err(AppError::Internal)?;
        app.uuid.clone()
    } else {
        tracing::error!(project = %request.project_name, "no existing Coolify app found");
        return Err(AppError::Internal(
            "Creating new Coolify applications not yet implemented. Create the app in Coolify UI first.".to_string(),
        ));
    };

    let root_domain = creds
        .root_domain
        .as_ref()
        .ok_or_else(|| AppError::Internal("Root domain not configured".to_string()))?;
    let domain = format!("{}.{}", request.subdomain, root_domain);
    let url = format!("https://{domain}");

    tracing::info!(url = %url, uuid = %uuid, "deployment complete");
    Ok(DeployResult {
        url,
        coolify_uuid: uuid,
    })
}

#[derive_handlers(tauri_command, axum_handler)]
pub async fn list_deployed_services() -> Result<Vec<serde_json::Value>, AppError> {
    tracing::debug!("listing deployed services");
    let creds = tokio::task::spawn_blocking(DeployCredentials::load)
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
        .map_err(AppError::Internal)?;
    if !creds.is_provisioned() {
        tracing::warn!("credentials not provisioned, returning empty service list");
        return Ok(vec![]);
    }

    let coolify = CoolifyClient::new(
        creds
            .coolify_url
            .as_ref()
            .ok_or_else(|| AppError::Internal("Coolify URL not configured".to_string()))?,
        creds
            .coolify_api_key
            .as_ref()
            .ok_or_else(|| AppError::Internal("Coolify API key not configured".to_string()))?,
    );

    let apps = coolify
        .list_applications()
        .await
        .map_err(AppError::Internal)?;
    let result: Vec<serde_json::Value> = apps
        .iter()
        .map(|app| {
            serde_json::json!({
                "uuid": app.uuid,
                "name": app.name,
                "status": app.status,
                "fqdn": app.fqdn,
            })
        })
        .collect();

    Ok(result)
}
