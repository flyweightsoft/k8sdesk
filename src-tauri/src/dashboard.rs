//! Kubernetes Dashboard proxy with transparent HTTP→HTTPS bridging.
//!
//! Starts an in-process HTTP reverse proxy that:
//! 1. Discovers the kubernetes-dashboard pod in the cluster.
//! 2. Gets a ServiceAccount bearer token via the TokenRequest API.
//! 3. Binds a random local TCP port.
//! 4. On `GET /_bridge`: serves an auto-login HTML page that POSTs the token
//!    to the dashboard's `/api/v1/login` endpoint, receives the session cookie,
//!    and then redirects to the dashboard root.
//! 5. On all other requests: forwards via kube portforward. If the target port
//!    is HTTPS (8443/443/9443 or port name contains "https"/"tls"), the
//!    portforward stream is wrapped in TLS with cert verification disabled
//!    (dashboard uses self-signed certs).
//! 6. Opens the system browser to `http://127.0.0.1:{port}/_bridge`.
//!
//! No system-level tools (kubectl, helm, …) are required.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::watch;

use k8s_openapi::api::authentication::v1::{TokenRequest, TokenRequestSpec};
use k8s_openapi::api::core::v1::{Pod, Secret, ServiceAccount};
use k8s_openapi::api::rbac::v1::{ClusterRoleBinding, RoleRef, Subject};
use kube::{
    api::{ListParams, PostParams},
    Api, Client,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TlsError, SignatureScheme};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;

use crate::error::{AppError, AppResult};

// ── TLS: accept any certificate (dashboard uses self-signed) ─────────────────

#[derive(Debug)]
struct NoCertVerifier;

impl ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        use SignatureScheme::*;
        vec![
            RSA_PKCS1_SHA1,
            ECDSA_SHA1_Legacy,
            RSA_PKCS1_SHA256,
            ECDSA_NISTP256_SHA256,
            RSA_PKCS1_SHA384,
            ECDSA_NISTP384_SHA384,
            RSA_PKCS1_SHA512,
            ECDSA_NISTP521_SHA512,
            RSA_PSS_SHA256,
            RSA_PSS_SHA384,
            RSA_PSS_SHA512,
            ED25519,
            ED448,
        ]
    }
}

/// Build a TLS connector that skips certificate verification.
/// Safe here: traffic travels through a localhost portforward tunnel
/// to a pod we already authenticated to via the Kubernetes API.
fn make_tls_connector() -> TlsConnector {
    let config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("safe TLS versions are always valid")
    .dangerous()
    .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
    .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

// ── port helpers ──────────────────────────────────────────────────────────────

/// Returns `true` if the port / port-name indicates an HTTPS endpoint.
fn port_is_https(port: u16, name: Option<&str>) -> bool {
    if let Some(n) = name {
        let n = n.to_ascii_lowercase();
        if n.contains("https") || n.contains("tls") || n.contains("secure") {
            return true;
        }
    }
    matches!(port, 443 | 8443 | 9443)
}

// ── public surface ───────────────────────────────────────────────────────────

/// Registry kept in `AppState` tracking one proxy per cluster.
#[derive(Default)]
pub struct DashboardRegistry {
    inner: HashMap<uuid::Uuid, DashboardSession>,
}

impl DashboardRegistry {
    /// Insert or replace a session.
    pub fn insert(&mut self, cluster_id: uuid::Uuid, session: DashboardSession) {
        self.inner.insert(cluster_id, session);
    }

    /// Return the session if the proxy is still alive.
    pub fn get_alive(&mut self, cluster_id: uuid::Uuid) -> Option<DashboardSession> {
        let session = self.inner.get(&cluster_id)?;
        if session.alive.load(Ordering::Relaxed) {
            Some(session.clone())
        } else {
            self.inner.remove(&cluster_id);
            None
        }
    }

    /// Signal the proxy for `cluster_id` to shut down and remove it.
    pub fn stop(&mut self, cluster_id: uuid::Uuid) {
        if let Some(session) = self.inner.remove(&cluster_id) {
            // Sending `true` tells the acceptor loop to break.
            let _ = session.shutdown_tx.send(true);
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct DashboardSession {
    /// `http://127.0.0.1:{port}` – the local proxy root URL.
    pub url: String,
    /// Bearer token – shown in the UI as a manual fallback.
    pub token: String,
    /// True while the proxy acceptor task is alive.
    #[serde(skip)]
    pub alive: Arc<AtomicBool>,
    /// Send `true` to this channel to shut the proxy down.
    #[serde(skip)]
    pub shutdown_tx: watch::Sender<bool>,
}

/// Discover the dashboard, mint a token, start the proxy, open the browser.
pub async fn open(client: Client) -> AppResult<DashboardSession> {
    let (pod_name, namespace, pod_port, pod_https) = find_dashboard_pod(&client).await?;
    tracing::info!(pod = %pod_name, ns = %namespace, port = pod_port, https = pod_https, "found dashboard pod");

    let token = get_dashboard_token(&client, &namespace).await?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| AppError::Kube(format!("could not bind local port: {e}")))?;

    let local_port = listener
        .local_addr()
        .map_err(|e| AppError::Kube(e.to_string()))?
        .port();

    let alive = Arc::new(AtomicBool::new(true));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let pods_api: Api<Pod> = Api::namespaced(client, &namespace);
    let pod_name_arc = Arc::new(pod_name);
    let token_arc = Arc::new(token.clone());

    tokio::spawn(run_proxy(
        listener,
        pods_api,
        pod_name_arc,
        pod_port,
        pod_https,
        token_arc,
        alive.clone(),
        shutdown_rx,
    ));

    let bridge_url = format!("http://127.0.0.1:{}/_bridge", local_port);
    // Open browser in a blocking thread so we don't stall the async runtime.
    let url_clone = bridge_url.clone();
    tokio::task::spawn_blocking(move || open::that(url_clone))
        .await
        .map_err(|e| AppError::Kube(format!("thread join error: {e}")))?
        .map_err(|e| AppError::Kube(format!("failed to open browser: {e}")))?;

    tracing::info!(port = local_port, "dashboard proxy started, browser opened");

    Ok(DashboardSession {
        url: format!("http://127.0.0.1:{}", local_port),
        token,
        alive,
        shutdown_tx,
    })
}

// ── proxy task ───────────────────────────────────────────────────────────────

async fn run_proxy(
    listener: TcpListener,
    pods_api: Api<Pod>,
    pod_name: Arc<String>,
    pod_port: u16,
    pod_https: bool,
    token: Arc<String>,
    alive: Arc<AtomicBool>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            // Graceful shutdown signal.
            _ = shutdown_rx.changed() => {
                tracing::info!("dashboard proxy: shutdown requested");
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let pods = pods_api.clone();
                        let name = pod_name.clone();
                        let tok = token.clone();
                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_connection(stream, pods, &name, pod_port, pod_https, &tok).await
                            {
                                tracing::debug!("dashboard proxy: connection closed: {e}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("dashboard proxy: accept error: {e}");
                        break;
                    }
                }
            }
        }
    }
    alive.store(false, Ordering::Relaxed);
    tracing::info!("dashboard proxy: acceptor exited");
}

async fn handle_connection(
    mut browser: TcpStream,
    pods_api: Api<Pod>,
    pod_name: &str,
    pod_port: u16,
    pod_https: bool,
    token: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Read the HTTP request line (up to first \r\n) by consuming bytes.
    // We buffer it so we can replay it to the pod for non-bridge requests.
    let mut request_line: Vec<u8> = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        browser.read_exact(&mut byte).await?;
        request_line.push(byte[0]);
        if request_line.ends_with(b"\r\n") || request_line.len() >= 256 {
            break;
        }
    }

    if request_line.starts_with(b"GET /_bridge") {
        // Drain the remaining request headers, then serve the bridge page.
        consume_http_headers(&mut browser).await?;

        let html = bridge_html(token);
        let response = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/html; charset=utf-8\r\n\
             Content-Length: {len}\r\n\
             Cache-Control: no-store\r\n\
             Connection: close\r\n\
             \r\n\
             {html}",
            len = html.len(),
        );
        browser.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    // For every other request, create a portforward, replay the request line
    // we already consumed, then pipe bidirectionally.
    let mut pf = pods_api.portforward(pod_name, &[pod_port]).await?;
    let mut pod_stream = pf
        .take_stream(pod_port)
        .ok_or("portforward: no stream returned for requested port")?;

    if pod_https {
        let connector = make_tls_connector();
        let server_name = ServerName::try_from("kubernetes-dashboard")
            .expect("static server name is valid");
        let mut tls_stream = connector.connect(server_name, pod_stream).await?;
        // Replay the request line we already read from the browser.
        tls_stream.write_all(&request_line).await?;
        tokio::io::copy_bidirectional(&mut browser, &mut tls_stream).await?;
    } else {
        pod_stream.write_all(&request_line).await?;
        tokio::io::copy_bidirectional(&mut browser, &mut pod_stream).await?;
    }
    Ok(())
}

/// Read byte-by-byte until we see the end-of-headers sentinel `\r\n\r\n`.
async fn consume_http_headers(stream: &mut TcpStream) -> tokio::io::Result<()> {
    let mut byte = [0u8; 1];
    let mut tail = [0u8; 4];
    loop {
        match stream.read_exact(&mut byte).await {
            Ok(_) => {}
            Err(_) => break, // connection closed mid-headers – that's fine
        }
        tail.rotate_left(1);
        tail[3] = byte[0];
        if &tail == b"\r\n\r\n" {
            break;
        }
    }
    Ok(())
}

// ── auth-bridge HTML ─────────────────────────────────────────────────────────

fn bridge_html(token: &str) -> String {
    // JSON-encode the token so it is safe to embed inside JavaScript.
    let token_json = serde_json::to_string(token).unwrap_or_else(|_| {
        format!(
            "\"{}\"",
            token.replace('\\', "\\\\").replace('"', "\\\"")
        )
    });

    // NOTE: every literal `{` / `}` in the template that belongs to HTML/CSS/JS
    // must be doubled (`{{` / `}}`) because we are inside a Rust format string.
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>k8sdesk – Opening Dashboard…</title>
  <style>
    *, *::before, *::after {{ box-sizing: border-box; }}
    body {{
      font-family: system-ui, -apple-system, sans-serif;
      display: flex; align-items: center; justify-content: center;
      height: 100vh; margin: 0;
      background: #0f172a; color: #e2e8f0;
    }}
    .card {{ text-align: center; padding: 2rem; max-width: 480px; }}
    h1 {{ font-size: 1.25rem; margin: 0 0 .5rem; }}
    p  {{ margin: .25rem 0; color: #94a3b8; font-size: .9rem; }}
    .spinner {{
      width: 44px; height: 44px;
      border: 4px solid rgba(255,255,255,.15);
      border-top-color: #38bdf8;
      border-radius: 50%;
      animation: spin .8s linear infinite;
      margin: 1.25rem auto;
    }}
    @keyframes spin {{ to {{ transform: rotate(360deg); }} }}
    .err {{
      color: #fca5a5; margin-top: 1rem;
      white-space: pre-wrap; word-break: break-all;
      font-size: .8rem; text-align: left;
      background: rgba(239,68,68,.1); padding: .75rem; border-radius: .5rem;
    }}
    .hidden {{ display: none !important; }}
  </style>
</head>
<body>
  <div class="card">
    <h1>Kubernetes Dashboard</h1>
    <p>Authenticating — please wait…</p>
    <div class="spinner" id="spinner"></div>
    <div id="err" class="err hidden"></div>
  </div>
  <script>
    (async function () {{
      const BEARER = {token_json};

      function setCookie(name, value) {{
        // Session cookie, path=/, Lax so it travels with same-origin nav.
        document.cookie =
          name + '=' + value + '; path=/; SameSite=Lax';
      }}

      async function getCsrf() {{
        const r = await fetch('/api/v1/csrftoken/login', {{
          method: 'GET',
          credentials: 'include',
          headers: {{ 'Accept': 'application/json' }}
        }});
        if (!r.ok) {{
          const body = await r.text().catch(() => '');
          throw new Error('CSRF fetch ' + r.status + (body ? ': ' + body : ''));
        }}
        const j = await r.json();
        const t = j && (j.token || j.Token);
        if (!t) throw new Error('CSRF response missing token');
        return t;
      }}

      async function login(csrf) {{
        const r = await fetch('/api/v1/login', {{
          method: 'POST',
          credentials: 'include',
          headers: {{
            'Content-Type': 'application/json',
            'X-CSRF-TOKEN': csrf
          }},
          body: JSON.stringify({{ token: BEARER }})
        }});
        if (!r.ok) {{
          const body = await r.text().catch(() => '');
          throw new Error('login ' + r.status + (body ? ': ' + body : ''));
        }}
        let data = null;
        try {{ data = await r.json(); }} catch (_) {{ /* may be empty */ }}
        return data || {{}};
      }}

      try {{
        const csrf = await getCsrf();
        const data = await login(csrf);

        // Extract the JWE session token, regardless of which Dashboard
        // version we're talking to.
        const jwe =
          data.jweToken ||
          data.JWEToken ||
          data.token ||
          null;

        if (jwe) {{
          // 1) Plant the cookie the Dashboard SPA looks for.
          setCookie('jweToken', jwe);
          // 2) Some forks read it from localStorage instead.
          try {{ localStorage.setItem('jweToken', jwe); }} catch (_) {{}}
        }}

        // Some installs accept the bearer token directly in
        // the `Authorization` header for every API call. Stash it for
        // SPAs that read this on boot.
        try {{ localStorage.setItem('id_token', BEARER); }} catch (_) {{}}

        // Go to the overview, not `/`, so the SPA bypasses its
        // initial /#/login redirect logic.
        window.location.replace('/#/workloads?namespace=_all');
      }} catch (e) {{
        document.getElementById('spinner').classList.add('hidden');
        const el = document.getElementById('err');
        el.textContent =
          String(e) +
          '\n\nAuto-login failed. You can still log in manually:\n' +
          'copy the token shown in k8sdesk and paste it on the dashboard login page.';
        el.classList.remove('hidden');
      }}
    }})();
  </script>
</body>
</html>"##,
        token_json = token_json
    )
}

// ── pod discovery ─────────────────────────────────────────────────────────────

/// Discover a running kubernetes-dashboard pod.
/// Returns `(pod_name, namespace, container_port, is_https)`.
async fn find_dashboard_pod(client: &Client) -> AppResult<(String, String, u16, bool)> {
    // Namespaces to probe, in priority order.
    let namespaces = ["kubernetes-dashboard", "kube-system", "default"];
    // Label selectors used by common installation methods.
    let selectors = [
        "app.kubernetes.io/name=kubernetes-dashboard",
        "app=kubernetes-dashboard",
        "k8s-app=kubernetes-dashboard",
        "app.kubernetes.io/part-of=kubernetes-dashboard",
    ];

    for ns in &namespaces {
        let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
        for sel in &selectors {
            let lp = ListParams::default().labels(sel);
            let list = match pods.list(&lp).await {
                Ok(l) => l,
                Err(_) => continue,
            };
            for pod in list.items {
                let phase = pod
                    .status
                    .as_ref()
                    .and_then(|s| s.phase.as_deref())
                    .unwrap_or_default();
                if phase != "Running" {
                    continue;
                }
                let Some(name) = pod.metadata.name else {
                    continue;
                };

                let port_list = pod
                    .spec
                    .as_ref()
                    .and_then(|s| s.containers.first())
                    .and_then(|c| c.ports.as_ref())
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);

                // Prefer plain-HTTP ports so no TLS is needed;
                // fall back to whatever is declared first.
                let chosen = port_list
                    .iter()
                    .find(|p| matches!(p.container_port, 9090 | 8080 | 80))
                    .or_else(|| port_list.first());

                let (port, https) = chosen
                    .map(|p| {
                        let port = p.container_port as u16;
                        let https = port_is_https(port, p.name.as_deref());
                        (port, https)
                    })
                    .unwrap_or((9090, false));

                return Ok((name, ns.to_string(), port, https));
            }
        }
    }

    Err(AppError::Kube(
        "kubernetes-dashboard pod not found. \
         Make sure the dashboard is deployed and running in the \
         'kubernetes-dashboard' or 'kube-system' namespace."
            .into(),
    ))
}

// ── token acquisition ─────────────────────────────────────────────────────────

const K8SDESK_SA: &str = "k8sdesk-admin";
const K8SDESK_CRB: &str = "k8sdesk-admin";

/// Get a cluster-admin bearer token for the Kubernetes Dashboard.
///
/// Strategy:
/// 1. Try existing well-known admin ServiceAccounts (`admin-user`,
///    `dashboard-admin`) — that's the canonical setup users follow.
/// 2. If none exist, ensure our own `k8sdesk-admin` SA + cluster-admin
///    ClusterRoleBinding are present, then mint a token for it.
/// 3. As a last resort, fall back to the dashboard's own SA (limited RBAC,
///    user will see a mostly-empty UI but at least not 401).
async fn get_dashboard_token(client: &Client, namespace: &str) -> AppResult<String> {
    // 1. Try canonical admin SAs first.
    for sa_name in &["admin-user", "dashboard-admin"] {
        if let Some(tok) = try_mint_token(client, namespace, sa_name).await {
            tracing::info!(sa = sa_name, ns = namespace, "minted dashboard token");
            return Ok(tok);
        }
    }

    // 2. Self-provision a cluster-admin SA the first time, then mint.
    match ensure_k8sdesk_admin(client, namespace).await {
        Ok(()) => {
            if let Some(tok) = try_mint_token(client, namespace, K8SDESK_SA).await {
                tracing::info!(
                    sa = K8SDESK_SA,
                    ns = namespace,
                    "minted token for self-provisioned admin SA"
                );
                return Ok(tok);
            }
        }
        Err(e) => {
            tracing::warn!("could not provision {K8SDESK_SA}: {e}");
        }
    }

    // 3. Fallback to dashboard SA (limited RBAC but at least not 401).
    if let Some(tok) = try_mint_token(client, namespace, "kubernetes-dashboard").await {
        tracing::warn!(
            "using kubernetes-dashboard SA token \
             — RBAC may restrict what is visible in the UI"
        );
        return Ok(tok);
    }

    // 4. Legacy SA-token Secret fallback.
    let secrets_api: Api<Secret> = Api::namespaced(client.clone(), namespace);
    if let Ok(list) = secrets_api.list(&ListParams::default()).await {
        for secret in list.items {
            if secret.type_.as_deref().unwrap_or_default()
                != "kubernetes.io/service-account-token"
            {
                continue;
            }
            if let Some(data) = secret.data {
                if let Some(token_bytes) = data.get("token") {
                    if let Ok(t) = String::from_utf8(token_bytes.0.clone()) {
                        return Ok(t);
                    }
                }
            }
        }
    }

    Err(AppError::Kube(
        "Could not obtain a dashboard token. \
         The cluster credentials configured in k8sdesk lack permission to \
         create ServiceAccounts / ClusterRoleBindings or call the TokenRequest API."
            .into(),
    ))
}

/// Try to mint a TokenRequest for `sa_name`. Returns `None` on any failure.
async fn try_mint_token(client: &Client, namespace: &str, sa_name: &str) -> Option<String> {
    let sa_api: Api<ServiceAccount> = Api::namespaced(client.clone(), namespace);
    if sa_api.get(sa_name).await.is_err() {
        return None;
    }

    let tr_body = TokenRequest {
        metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
            name: Some(sa_name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        spec: TokenRequestSpec {
            audiences: vec![],
            expiration_seconds: Some(7200),
            bound_object_ref: None,
        },
        status: None,
    };

    let body = serde_json::to_vec(&tr_body).ok()?;
    let result = sa_api
        .create_subresource::<TokenRequest>("token", sa_name, &PostParams::default(), body)
        .await
        .ok()?;
    result.status.map(|s| s.token)
}

/// Idempotently create the `k8sdesk-admin` ServiceAccount in `namespace`
/// and bind it to the built-in `cluster-admin` ClusterRole.
async fn ensure_k8sdesk_admin(client: &Client, namespace: &str) -> AppResult<()> {
    // ServiceAccount.
    let sa_api: Api<ServiceAccount> = Api::namespaced(client.clone(), namespace);
    if sa_api.get(K8SDESK_SA).await.is_err() {
        let sa = ServiceAccount {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(K8SDESK_SA.to_string()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        sa_api
            .create(&PostParams::default(), &sa)
            .await
            .map_err(|e| AppError::Kube(format!("create ServiceAccount: {e}")))?;
        tracing::info!(sa = K8SDESK_SA, ns = namespace, "created ServiceAccount");
    }

    // ClusterRoleBinding (cluster-admin).
    let crb_api: Api<ClusterRoleBinding> = Api::all(client.clone());
    if crb_api.get(K8SDESK_CRB).await.is_err() {
        let crb = ClusterRoleBinding {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(K8SDESK_CRB.to_string()),
                ..Default::default()
            },
            role_ref: RoleRef {
                api_group: "rbac.authorization.k8s.io".to_string(),
                kind: "ClusterRole".to_string(),
                name: "cluster-admin".to_string(),
            },
            subjects: Some(vec![Subject {
                kind: "ServiceAccount".to_string(),
                name: K8SDESK_SA.to_string(),
                namespace: Some(namespace.to_string()),
                api_group: None,
            }]),
        };
        crb_api
            .create(&PostParams::default(), &crb)
            .await
            .map_err(|e| AppError::Kube(format!("create ClusterRoleBinding: {e}")))?;
        tracing::info!(
            crb = K8SDESK_CRB,
            sa = K8SDESK_SA,
            "created cluster-admin binding"
        );
    }

    Ok(())
}
