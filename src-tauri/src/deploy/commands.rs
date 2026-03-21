use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ProjectSignals {
    pub has_dockerfile: bool,
    pub has_package_json: bool,
    pub has_vite_config: bool,
    pub has_start_script: bool,
    pub has_pyproject: bool,
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
