//! Builds `kube::Client`s entirely from in-memory `ClusterRecord`s.
//! Never reads `$KUBECONFIG`, `~/.kube/config`, or any other system file.

use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use kube::{
    config::{
        AuthInfo, Cluster as KCluster, Context, KubeConfigOptions, Kubeconfig, NamedAuthInfo,
        NamedCluster, NamedContext,
    },
    Client, Config,
};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::store::{Auth, ClusterRecord};

#[derive(Default)]
pub struct ClientCache {
    inner: HashMap<Uuid, Client>,
}

impl ClientCache {
    pub fn invalidate(&mut self, id: Uuid) {
        self.inner.remove(&id);
    }

    pub async fn get(&mut self, rec: &ClusterRecord) -> AppResult<Client> {
        if let Some(c) = self.inner.get(&rec.id) {
            return Ok(c.clone());
        }
        let client = build_client(rec).await?;
        self.inner.insert(rec.id, client.clone());
        Ok(client)
    }
}

/// Construct a `Kubeconfig` purely from in-memory data and turn it into a `Client`.
pub async fn build_client(rec: &ClusterRecord) -> AppResult<Client> {
    let cluster_name = "k8sdesk-cluster".to_string();
    let user_name = "k8sdesk-user".to_string();
    let context_name = "k8sdesk-context".to_string();

    let cluster = KCluster {
        server: Some(rec.api_server.clone()),
        certificate_authority: None,
        certificate_authority_data: if rec.ca_pem.is_empty() {
            None
        } else {
            Some(B64.encode(rec.ca_pem.as_bytes()))
        },
        insecure_skip_tls_verify: Some(rec.insecure_skip_tls_verify),
        proxy_url: None,
        tls_server_name: None,
        extensions: None,
    };

    let auth_info = match &rec.auth {
        Auth::BearerToken { token } => AuthInfo {
            token: Some(token.clone().into()),
            ..Default::default()
        },
        Auth::ClientCert { cert_pem, key_pem } => AuthInfo {
            client_certificate_data: Some(B64.encode(cert_pem.as_bytes())),
            client_key_data: Some(B64.encode(key_pem.as_bytes()).into()),
            ..Default::default()
        },
    };

    let kc = Kubeconfig {
        preferences: None,
        clusters: vec![NamedCluster {
            name: cluster_name.clone(),
            cluster: Some(cluster),
        }],
        auth_infos: vec![NamedAuthInfo {
            name: user_name.clone(),
            auth_info: Some(auth_info),
        }],
        contexts: vec![NamedContext {
            name: context_name.clone(),
            context: Some(Context {
                cluster: cluster_name,
                user: user_name,
                namespace: Some(rec.default_namespace.clone()),
                extensions: None,
            }),
        }],
        current_context: Some(context_name),
        extensions: None,
        kind: None,
        api_version: None,
    };

    let cfg = Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default())
        .await
        .map_err(AppError::from)?;
    Client::try_from(cfg).map_err(AppError::from)
}
