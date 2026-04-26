//! Destructive-command classifier and one-shot HMAC confirmation tokens.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde::Serialize;
use sha2::Sha256;
use uuid::Uuid;

use crate::dsl::parser::Command;
use crate::error::{AppError, AppResult};
use crate::store::{ClusterRecord, Environment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Safe,
    Destructive,
}

pub fn classify(cmd: &Command) -> Severity {
    match cmd {
        Command::Get { .. }
        | Command::Describe { .. }
        | Command::Logs { .. }
        | Command::Help => Severity::Safe,
        Command::Delete { .. }
        | Command::Scale { .. }
        | Command::RolloutRestart { .. }
        | Command::Apply { .. } => Severity::Destructive,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfirmationRequest {
    pub challenge_id: String,
    pub cluster_id: Uuid,
    pub cluster_name: String,
    pub environment: Environment,
    pub action_summary: String,
    /// True if the user must type the cluster name to confirm.
    pub require_typed_name: bool,
}

pub fn summarize(cmd: &Command, namespace: &str) -> String {
    match cmd {
        Command::Delete { resource, name } => {
            format!("DELETE {:?} '{}' in namespace '{}'", resource, name, namespace)
        }
        Command::Scale { resource, name, replicas } => format!(
            "SCALE {:?} '{}' to {} replicas in namespace '{}'",
            resource, name, replicas, namespace
        ),
        Command::RolloutRestart { resource, name } => format!(
            "ROLLOUT RESTART {:?} '{}' in namespace '{}'",
            resource, name, namespace
        ),
        Command::Apply { paste_id } => format!(
            "APPLY pasted manifest '{}' into namespace '{}'",
            paste_id, namespace
        ),
        _ => "safe operation".into(),
    }
}

// ---------- one-shot tokens ----------

const TTL: Duration = Duration::from_secs(30);

struct Pending {
    cluster_id: Uuid,
    namespace: String,
    command_fingerprint: [u8; 32],
    created: Instant,
}

pub struct PendingConfirmations {
    hmac_key: [u8; 32],
    items: HashMap<String, Pending>,
}

impl Default for PendingConfirmations {
    fn default() -> Self {
        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        Self {
            hmac_key: k,
            items: HashMap::new(),
        }
    }
}

impl PendingConfirmations {
    pub fn issue(&mut self, cluster_id: Uuid, namespace: &str, cmd: &Command) -> String {
        self.gc();
        let challenge_id = Uuid::new_v4().to_string();
        self.items.insert(
            challenge_id.clone(),
            Pending {
                cluster_id,
                namespace: namespace.to_string(),
                command_fingerprint: fingerprint(&self.hmac_key, cluster_id, namespace, cmd),
                created: Instant::now(),
            },
        );
        challenge_id
    }

    /// Consume a token, verifying it matches the (cluster, namespace, command) tuple.
    pub fn consume(
        &mut self,
        challenge_id: &str,
        cluster_id: Uuid,
        namespace: &str,
        cmd: &Command,
    ) -> AppResult<()> {
        self.gc();
        let entry = self
            .items
            .remove(challenge_id)
            .ok_or(AppError::BadConfirmation)?;
        if entry.created.elapsed() > TTL
            || entry.cluster_id != cluster_id
            || entry.namespace != namespace
            || entry.command_fingerprint != fingerprint(&self.hmac_key, cluster_id, namespace, cmd)
        {
            return Err(AppError::BadConfirmation);
        }
        Ok(())
    }

    fn gc(&mut self) {
        self.items.retain(|_, p| p.created.elapsed() <= TTL);
    }
}

fn fingerprint(key: &[u8; 32], cluster: Uuid, ns: &str, cmd: &Command) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac");
    mac.update(cluster.as_bytes());
    mac.update(b"|");
    mac.update(ns.as_bytes());
    mac.update(b"|");
    let serialized = format!("{:?}", cmd);
    mac.update(serialized.as_bytes());
    let out = mac.finalize().into_bytes();
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&out);
    arr
}

pub fn build_confirmation_request(
    rec: &ClusterRecord,
    namespace: &str,
    cmd: &Command,
    challenge_id: String,
) -> ConfirmationRequest {
    ConfirmationRequest {
        challenge_id,
        cluster_id: rec.id,
        cluster_name: rec.name.clone(),
        environment: rec.environment,
        action_summary: summarize(cmd, namespace),
        require_typed_name: rec.environment.is_prod(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::parser::{Command, Resource};

    #[test]
    fn classifies_destructive() {
        assert_eq!(
            classify(&Command::Delete {
                resource: Resource::Pods,
                name: "x".into()
            }),
            Severity::Destructive
        );
        assert_eq!(
            classify(&Command::Scale {
                resource: Resource::Deployments,
                name: "x".into(),
                replicas: 0
            }),
            Severity::Destructive
        );
        assert_eq!(
            classify(&Command::RolloutRestart {
                resource: Resource::Deployments,
                name: "x".into()
            }),
            Severity::Destructive
        );
        assert_eq!(
            classify(&Command::Apply {
                paste_id: "p".into()
            }),
            Severity::Destructive
        );
    }

    #[test]
    fn classifies_safe() {
        assert_eq!(
            classify(&Command::Get {
                resource: Resource::Pods,
                name: None
            }),
            Severity::Safe
        );
        assert_eq!(classify(&Command::Help), Severity::Safe);
    }

    #[test]
    fn confirmation_token_round_trip() {
        let mut p = PendingConfirmations::default();
        let cid = Uuid::new_v4();
        let cmd = Command::Delete {
            resource: Resource::Pods,
            name: "x".into(),
        };
        let tok = p.issue(cid, "ns", &cmd);
        // wrong command rejected
        assert!(p
            .consume(
                &tok,
                cid,
                "ns",
                &Command::Delete {
                    resource: Resource::Pods,
                    name: "y".into()
                }
            )
            .is_err());
        // correct command accepted
        let tok = p.issue(cid, "ns", &cmd);
        assert!(p.consume(&tok, cid, "ns", &cmd).is_ok());
        // single-use: reusing fails
        assert!(p.consume(&tok, cid, "ns", &cmd).is_err());
    }
}
