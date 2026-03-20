use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ProjectSignals {
    pub has_dockerfile: bool,
    pub has_package_json: bool,
    pub has_vite_config: bool,
    pub has_start_script: bool,
    pub has_pyproject: bool,
}

// [migrated to generated.rs]
pub async fn detect_project_type(repo_path: String) -> Result<ProjectSignals, String> {
    tokio::task::spawn_blocking(move || {
        crate::service::detect_project_type_blocking(&repo_path).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

// [migrated to generated.rs]
pub async fn get_deploy_credentials() -> Result<super::credentials::DeployCredentials, String> {
    tokio::task::spawn_blocking(|| {
        crate::service::get_deploy_credentials_blocking().map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

// [migrated to generated.rs]
pub async fn save_deploy_credentials(
    credentials: super::credentials::DeployCredentials,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        crate::service::save_deploy_credentials_blocking(credentials).map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

// [migrated to generated.rs]
pub async fn is_deploy_provisioned() -> Result<bool, String> {
    tokio::task::spawn_blocking(|| {
        crate::service::is_deploy_provisioned_blocking().map_err(Into::into)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
pub struct DeployRequest {
    pub project_name: String,
    pub repo_path: String,
    pub subdomain: String,
    pub project_type: String,
}

#[derive(Serialize)]
pub struct DeployResult {
    pub url: String,
    pub coolify_uuid: String,
}

// [migrated to generated.rs]
pub async fn deploy_project(request: DeployRequest) -> Result<DeployResult, String> {
    crate::service::deploy_project(request)
        .await
        .map_err(Into::into)
}

// [migrated to generated.rs]
pub async fn list_deployed_services() -> Result<Vec<serde_json::Value>, String> {
    crate::service::list_deployed_services()
        .await
        .map_err(Into::into)
}
