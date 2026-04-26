//! Tauri IPC command surface.
//!
//! The frontend is restricted to these calls only. None of them accept a
//! `KUBECONFIG` path or arbitrary shell input.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};
use uuid::Uuid;

use crate::dashboard;
use crate::dsl::executor::{self, CommandOutput};
use crate::dsl::parser;
use crate::error::{AppError, AppResult};
use crate::safety::{self, ConfirmationRequest, Severity};
use crate::store::{Auth, ClusterRecord, Environment, RedactedCluster};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ClusterInput {
    pub name: String,
    pub environment: Environment,
    pub api_server: String,
    pub ca_pem: String,
    pub auth: Auth,
    pub default_namespace: String,
    pub insecure_skip_tls_verify: bool,
}

impl From<ClusterInput> for ClusterRecord {
    fn from(i: ClusterInput) -> Self {
        ClusterRecord {
            id: Uuid::nil(),
            name: i.name,
            environment: i.environment,
            api_server: i.api_server,
            ca_pem: i.ca_pem,
            auth: i.auth,
            default_namespace: if i.default_namespace.trim().is_empty() {
                "default".into()
            } else {
                i.default_namespace
            },
            insecure_skip_tls_verify: i.insecure_skip_tls_verify,
        }
    }
}

#[tauri::command]
pub async fn cluster_list(state: State<'_, Arc<AppState>>) -> AppResult<Vec<RedactedCluster>> {
    Ok(state.store.lock().await.list())
}

#[tauri::command]
pub async fn cluster_add(
    state: State<'_, Arc<AppState>>,
    input: ClusterInput,
) -> AppResult<RedactedCluster> {
    let rec: ClusterRecord = input.into();
    state.store.lock().await.add(rec)
}

#[tauri::command]
pub async fn cluster_update(
    state: State<'_, Arc<AppState>>,
    id: Uuid,
    input: ClusterInput,
) -> AppResult<RedactedCluster> {
    // Load the stored record so we can preserve secrets the user didn't re-submit.
    let mut rec = {
        let store = state.store.lock().await;
        store.get(id)?.clone()
    };

    // Update non-secret fields unconditionally.
    rec.name = input.name;
    rec.environment = input.environment;
    rec.api_server = input.api_server;
    rec.default_namespace = if input.default_namespace.trim().is_empty() {
        "default".into()
    } else {
        input.default_namespace
    };
    rec.insecure_skip_tls_verify = input.insecure_skip_tls_verify;

    // Only replace CA if a new value was provided; otherwise keep the stored one.
    if !input.ca_pem.is_empty() {
        rec.ca_pem = input.ca_pem;
    }

    // Only replace auth credentials if new ones were provided.
    let replace_auth = match &input.auth {
        Auth::BearerToken { token } => !token.is_empty(),
        Auth::ClientCert { cert_pem, key_pem } => !cert_pem.is_empty() || !key_pem.is_empty(),
    };
    if replace_auth {
        rec.auth = input.auth;
    }

    let red = state.store.lock().await.update(rec)?;
    state.clients.lock().await.invalidate(id);
    Ok(red)
}

#[tauri::command]
pub async fn cluster_delete(state: State<'_, Arc<AppState>>, id: Uuid) -> AppResult<()> {
    state.store.lock().await.delete(id)?;
    state.clients.lock().await.invalidate(id);
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct KubeconfigImport {
    pub yaml: String,
    pub name: String,
    pub environment: Environment,
}

/// Parse a pasted kubeconfig and import the *current-context* (or first context)
/// as a cluster record. Rejects exec / auth-provider plugins outright.
#[tauri::command]
pub async fn cluster_import_kubeconfig(
    state: State<'_, Arc<AppState>>,
    input: KubeconfigImport,
) -> AppResult<RedactedCluster> {
    let rec = parse_kubeconfig(&input.yaml, &input.name, input.environment)?;
    state.store.lock().await.add(rec)
}

/// Update an existing cluster by replacing its credentials with those parsed
/// from a pasted kubeconfig. Name and environment come from the form.
#[tauri::command]
pub async fn cluster_update_from_kubeconfig(
    state: State<'_, Arc<AppState>>,
    id: Uuid,
    input: KubeconfigImport,
) -> AppResult<RedactedCluster> {
    let mut rec = parse_kubeconfig(&input.yaml, &input.name, input.environment)?;
    rec.id = id;
    let red = state.store.lock().await.update(rec)?;
    state.clients.lock().await.invalidate(id);
    Ok(red)
}

#[tauri::command]
pub async fn namespace_list(
    state: State<'_, Arc<AppState>>,
    cluster_id: Uuid,
) -> AppResult<Vec<String>> {
    let rec = {
        let store = state.store.lock().await;
        store.get(cluster_id)?.clone()
    };
    let client = state.clients.lock().await.get(&rec).await?;
    let api: kube::Api<k8s_openapi::api::core::v1::Namespace> = kube::Api::all(client);
    let list = api.list(&kube::api::ListParams::default()).await?;
    Ok(list
        .items
        .into_iter()
        .filter_map(|n| n.metadata.name)
        .collect())
}

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    pub cluster_id: Uuid,
    pub namespace: String,
    pub command: String,
    /// Optional confirmation token returned by an earlier execute call.
    pub confirmation: Option<String>,
    /// Optional inline YAML for `apply` (matched by paste_id in the command).
    pub apply_body: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecuteResponse {
    Output { severity: Severity, output: CommandOutput },
    NeedsConfirmation { request: ConfirmationRequest },
}

#[tauri::command]
pub async fn dsl_execute(
    state: State<'_, Arc<AppState>>,
    req: ExecuteRequest,
) -> AppResult<ExecuteResponse> {
    let parsed = parser::parse(&req.command)?;
    let severity = safety::classify(&parsed);

    let rec = {
        let store = state.store.lock().await;
        store.get(req.cluster_id)?.clone()
    };

    if matches!(severity, Severity::Destructive) {
        match req.confirmation {
            None => {
                let challenge =
                    state
                        .pending
                        .lock()
                        .await
                        .issue(rec.id, &req.namespace, &parsed);
                let request =
                    safety::build_confirmation_request(&rec, &req.namespace, &parsed, challenge);
                return Ok(ExecuteResponse::NeedsConfirmation { request });
            }
            Some(token) => {
                state
                    .pending
                    .lock()
                    .await
                    .consume(&token, rec.id, &req.namespace, &parsed)?;
            }
        }
    }

    let client = state.clients.lock().await.get(&rec).await?;
    let output = executor::execute(client, &req.namespace, parsed, req.apply_body).await?;

    tracing::info!(
        cluster = %rec.name,
        env = ?rec.environment,
        ns = %req.namespace,
        severity = ?severity,
        "command executed"
    );

    Ok(ExecuteResponse::Output { severity, output })
}

// ---------- kubeconfig parsing ----------

#[derive(Debug, Deserialize)]
struct KcRoot {
    clusters: Vec<KcNamedCluster>,
    users: Vec<KcNamedUser>,
    contexts: Vec<KcNamedContext>,
    #[serde(rename = "current-context")]
    current_context: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KcNamedCluster {
    name: String,
    cluster: KcCluster,
}
#[derive(Debug, Deserialize)]
struct KcCluster {
    server: String,
    #[serde(rename = "certificate-authority-data")]
    certificate_authority_data: Option<String>,
    #[serde(rename = "insecure-skip-tls-verify")]
    insecure_skip_tls_verify: Option<bool>,
}
#[derive(Debug, Deserialize)]
struct KcNamedUser {
    name: String,
    user: serde_yaml::Value,
}
#[derive(Debug, Deserialize)]
struct KcNamedContext {
    name: String,
    context: KcContext,
}
#[derive(Debug, Deserialize)]
struct KcContext {
    cluster: String,
    user: String,
    namespace: Option<String>,
}

fn parse_kubeconfig(yaml: &str, name: &str, env: Environment) -> AppResult<ClusterRecord> {
    let root: KcRoot = serde_yaml::from_str(yaml)
        .map_err(|e| AppError::KubeconfigRejected(format!("invalid yaml: {}", e)))?;

    let ctx_name = root
        .current_context
        .clone()
        .or_else(|| root.contexts.first().map(|c| c.name.clone()))
        .ok_or_else(|| AppError::KubeconfigRejected("no contexts found".into()))?;

    let ctx = root
        .contexts
        .iter()
        .find(|c| c.name == ctx_name)
        .ok_or_else(|| AppError::KubeconfigRejected("current-context not in contexts".into()))?;

    let cluster = root
        .clusters
        .iter()
        .find(|c| c.name == ctx.context.cluster)
        .ok_or_else(|| AppError::KubeconfigRejected("cluster ref not found".into()))?;

    let user = root
        .users
        .iter()
        .find(|u| u.name == ctx.context.user)
        .ok_or_else(|| AppError::KubeconfigRejected("user ref not found".into()))?;

    // Reject auth plugins outright.
    let user_map = user
        .user
        .as_mapping()
        .ok_or_else(|| AppError::KubeconfigRejected("user entry malformed".into()))?;

    if user_map.contains_key(&serde_yaml::Value::String("exec".into())) {
        return Err(AppError::KubeconfigRejected(
            "exec auth plugins are not supported".into(),
        ));
    }
    if user_map.contains_key(&serde_yaml::Value::String("auth-provider".into())) {
        return Err(AppError::KubeconfigRejected(
            "auth-provider plugins are not supported".into(),
        ));
    }

    let auth = if let Some(token) = user_map.get(&serde_yaml::Value::String("token".into())) {
        Auth::BearerToken {
            token: token
                .as_str()
                .ok_or_else(|| AppError::KubeconfigRejected("token must be a string".into()))?
                .to_string(),
        }
    } else if let (Some(cd), Some(kd)) = (
        user_map.get(&serde_yaml::Value::String(
            "client-certificate-data".into(),
        )),
        user_map.get(&serde_yaml::Value::String("client-key-data".into())),
    ) {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let cert = B64
            .decode(cd.as_str().unwrap_or("").as_bytes())
            .map_err(|_| AppError::KubeconfigRejected("bad client-certificate-data".into()))?;
        let key = B64
            .decode(kd.as_str().unwrap_or("").as_bytes())
            .map_err(|_| AppError::KubeconfigRejected("bad client-key-data".into()))?;
        Auth::ClientCert {
            cert_pem: String::from_utf8(cert)
                .map_err(|_| AppError::KubeconfigRejected("cert not UTF-8 PEM".into()))?,
            key_pem: String::from_utf8(key)
                .map_err(|_| AppError::KubeconfigRejected("key not UTF-8 PEM".into()))?,
        }
    } else {
        return Err(AppError::KubeconfigRejected(
            "user must have a static token or client-certificate-data + client-key-data".into(),
        ));
    };

    let ca_pem = if let Some(b64) = &cluster.cluster.certificate_authority_data {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let bytes = B64
            .decode(b64.as_bytes())
            .map_err(|_| AppError::KubeconfigRejected("bad certificate-authority-data".into()))?;
        String::from_utf8(bytes)
            .map_err(|_| AppError::KubeconfigRejected("CA not UTF-8 PEM".into()))?
    } else {
        String::new()
    };

    Ok(ClusterRecord {
        id: Uuid::nil(),
        name: name.to_string(),
        environment: env,
        api_server: cluster.cluster.server.clone(),
        ca_pem,
        auth,
        default_namespace: ctx.context.namespace.clone().unwrap_or_else(|| "default".into()),
        insecure_skip_tls_verify: cluster.cluster.insecure_skip_tls_verify.unwrap_or(false),
    })
}

// ── Kubernetes Dashboard ──────────────────────────────────────────────────────

/// Open the Kubernetes Dashboard for the given cluster.
///
/// If a proxy is already running for this cluster, just reopens the browser
/// to the existing URL — no new port is allocated. Otherwise, starts a new
/// in-process proxy and opens the browser to the auto-login bridge page.
#[tauri::command]
pub async fn dashboard_open(
    state: State<'_, Arc<AppState>>,
    cluster_id: Uuid,
) -> AppResult<dashboard::DashboardSession> {
    // Reuse an existing live proxy instead of spawning a duplicate.
    if let Some(existing) = state.dashboards.lock().await.get_alive(cluster_id) {
        let url = format!("{}/_bridge", existing.url);
        tokio::task::spawn_blocking(move || open::that(url))
            .await
            .map_err(|e| AppError::Kube(format!("thread join error: {e}")))?
            .map_err(|e| AppError::Kube(format!("failed to open browser: {e}")))?;
        return Ok(existing);
    }

    let rec = {
        let store = state.store.lock().await;
        store.get(cluster_id)?.clone()
    };
    let client = state.clients.lock().await.get(&rec).await?;
    let session = dashboard::open(client).await?;
    state.dashboards.lock().await.insert(cluster_id, session.clone());
    Ok(session)
}

/// Returns the active dashboard session for the cluster, or `null` if the
/// proxy is not running.
#[tauri::command]
pub async fn dashboard_status(
    state: State<'_, Arc<AppState>>,
    cluster_id: Uuid,
) -> AppResult<Option<dashboard::DashboardSession>> {
    Ok(state.dashboards.lock().await.get_alive(cluster_id))
}

/// Stop the dashboard proxy for the given cluster.
#[tauri::command]
pub async fn dashboard_stop(
    state: State<'_, Arc<AppState>>,
    cluster_id: Uuid,
) -> AppResult<()> {
    state.dashboards.lock().await.stop(cluster_id);
    Ok(())
}

// ── Cluster folder mapping ────────────────────────────────────────────────────

/// Return the folder path assigned to a cluster, or null if none assigned.
/// Stored in `cluster-folders.json` in the app data directory (plain JSON, not encrypted).
#[tauri::command]
pub async fn cluster_folder_get(
    cluster_id: String,
    app_handle: tauri::AppHandle,
) -> AppResult<Option<String>> {
    let app_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Storage(e.to_string()))?;
    let config_path = app_dir.join("cluster-folders.json");
    if !config_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&config_path)?;
    let map: std::collections::HashMap<String, String> =
        serde_json::from_str(&content).unwrap_or_default();
    Ok(map.get(&cluster_id).cloned())
}

/// Assign a folder path to a cluster.
/// Creates or updates `cluster-folders.json` in the app data directory.
#[tauri::command]
pub async fn cluster_folder_set(
    cluster_id: String,
    folder_path: String,
    app_handle: tauri::AppHandle,
) -> AppResult<()> {
    let app_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Storage(e.to_string()))?;
    std::fs::create_dir_all(&app_dir)?;
    let config_path = app_dir.join("cluster-folders.json");
    let mut map: std::collections::HashMap<String, String> = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        std::collections::HashMap::new()
    };
    map.insert(cluster_id, folder_path);
    let json = serde_json::to_string_pretty(&map)?;
    std::fs::write(&config_path, json)?;
    Ok(())
}

/// Open a URL in the system default browser.
#[tauri::command]
pub async fn open_url(url: String) -> AppResult<()> {
    // Reject non-http(s) schemes as a safety measure.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::Kube("open_url: only http/https URLs are allowed".into()));
    }
    tokio::task::spawn_blocking(move || open::that(url))
        .await
        .map_err(|e| AppError::Kube(format!("thread join error: {e}")))?
        .map_err(|e| AppError::Kube(format!("failed to open browser: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_exec_plugin() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: c
clusters:
- name: cl
  cluster:
    server: https://x
    certificate-authority-data: dGVzdA==
users:
- name: u
  user:
    exec:
      command: aws
      apiVersion: client.authentication.k8s.io/v1beta1
contexts:
- name: c
  context: { cluster: cl, user: u, namespace: default }
"#;
        let r = parse_kubeconfig(yaml, "n", Environment::Dev);
        assert!(matches!(r, Err(AppError::KubeconfigRejected(_))));
    }

    #[test]
    fn imports_token_kubeconfig() {
        let yaml = r#"
apiVersion: v1
kind: Config
current-context: c
clusters:
- name: cl
  cluster:
    server: https://x
    certificate-authority-data: dGVzdA==
users:
- name: u
  user:
    token: abc
contexts:
- name: c
  context: { cluster: cl, user: u, namespace: kube-system }
"#;
        let r = parse_kubeconfig(yaml, "n", Environment::Staging).unwrap();
        assert_eq!(r.api_server, "https://x");
        assert_eq!(r.default_namespace, "kube-system");
        match r.auth {
            Auth::BearerToken { token } => assert_eq!(token, "abc"),
            _ => panic!("expected token"),
        }
    }
}
