//! Restricted command DSL.
//!
//! Users do **not** type `kubectl`. Allowed verbs are a small whitelist; any
//! flag that could redirect the command at a different cluster/context
//! (`--context`, `--kubeconfig`, `--server`, `--token`, …) is rejected.

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Pods,
    Deployments,
    Services,
    Nodes,
    Namespaces,
    ConfigMaps,
    Secrets,
    Events,
    ReplicaSets,
    StatefulSets,
    DaemonSets,
    Ingresses,
    Jobs,
    CronJobs,
}

impl Resource {
    pub fn parse(s: &str) -> AppResult<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "pod" | "pods" | "po" => Resource::Pods,
            "deploy" | "deployment" | "deployments" => Resource::Deployments,
            "svc" | "service" | "services" => Resource::Services,
            "node" | "nodes" | "no" => Resource::Nodes,
            "ns" | "namespace" | "namespaces" => Resource::Namespaces,
            "cm" | "configmap" | "configmaps" => Resource::ConfigMaps,
            "secret" | "secrets" => Resource::Secrets,
            "event" | "events" | "ev" => Resource::Events,
            "rs" | "replicaset" | "replicasets" => Resource::ReplicaSets,
            "sts" | "statefulset" | "statefulsets" => Resource::StatefulSets,
            "ds" | "daemonset" | "daemonsets" => Resource::DaemonSets,
            "ing" | "ingress" | "ingresses" => Resource::Ingresses,
            "job" | "jobs" => Resource::Jobs,
            "cj" | "cronjob" | "cronjobs" => Resource::CronJobs,
            other => return Err(AppError::Parse(format!("unknown resource '{}'", other))),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Get {
        resource: Resource,
        name: Option<String>,
    },
    Describe {
        resource: Resource,
        name: String,
    },
    Logs {
        pod: String,
        container: Option<String>,
        tail: Option<i64>,
        follow: bool,
    },
    Delete {
        resource: Resource,
        name: String,
    },
    Scale {
        resource: Resource, // deployments / statefulsets
        name: String,
        replicas: i32,
    },
    RolloutRestart {
        resource: Resource,
        name: String,
    },
    Apply {
        // Inline YAML body, supplied through a separate paste field;
        // referenced here by an opaque id.
        paste_id: String,
    },
    Help,
}

/// Tokens that, if present anywhere in the input, immediately reject it.
/// These represent attempts to escape the app-managed context.
const FORBIDDEN_TOKENS: &[&str] = &[
    "kubectl",
    "--kubeconfig",
    "--context",
    "--server",
    "--token",
    "--user",
    "--cluster",
    "--as",
    "--as-group",
    "--certificate-authority",
    "--client-certificate",
    "--client-key",
    "--insecure-skip-tls-verify",
    "exec",
    "cp",
    "port-forward",
    "proxy",
    "auth",
    "config",
];

/// Tokens equal to one of these (case-insensitive) at any position fail.
fn is_forbidden(tok: &str) -> bool {
    let lc = tok.to_ascii_lowercase();
    FORBIDDEN_TOKENS.iter().any(|f| *f == lc)
}

pub fn parse(input: &str) -> AppResult<Command> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(AppError::Parse("empty command".into()));
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();

    for t in &tokens {
        if is_forbidden(t) {
            return Err(AppError::Forbidden(format!(
                "token '{}' is not allowed; the cluster and namespace are managed by the app",
                t
            )));
        }
    }

    let verb = tokens[0].to_ascii_lowercase();
    let rest = &tokens[1..];

    match verb.as_str() {
        "help" | "?" => Ok(Command::Help),
        "get" | "ls" | "list" => parse_get(rest),
        "describe" | "desc" => parse_describe(rest),
        "logs" | "log" => parse_logs(rest),
        "delete" | "del" | "rm" => parse_delete(rest),
        "scale" => parse_scale(rest),
        "rollout" => parse_rollout(rest),
        "apply" => parse_apply(rest),
        other => Err(AppError::Parse(format!(
            "unknown verb '{}'. type 'help' for a list",
            other
        ))),
    }
}

fn parse_get(rest: &[&str]) -> AppResult<Command> {
    if rest.is_empty() {
        return Err(AppError::Parse("usage: get <resource> [name]".into()));
    }
    let resource = Resource::parse(rest[0])?;
    let name = rest.get(1).map(|s| s.to_string());
    Ok(Command::Get { resource, name })
}

fn parse_describe(rest: &[&str]) -> AppResult<Command> {
    if rest.len() < 2 {
        return Err(AppError::Parse("usage: describe <resource> <name>".into()));
    }
    Ok(Command::Describe {
        resource: Resource::parse(rest[0])?,
        name: rest[1].to_string(),
    })
}

fn parse_logs(rest: &[&str]) -> AppResult<Command> {
    if rest.is_empty() {
        return Err(AppError::Parse(
            "usage: logs <pod> [-c container] [--tail N] [-f]".into(),
        ));
    }
    let mut pod = None;
    let mut container = None;
    let mut tail = None;
    let mut follow = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            "-c" | "--container" => {
                i += 1;
                container = Some(
                    rest.get(i)
                        .ok_or_else(|| AppError::Parse("expected container name".into()))?
                        .to_string(),
                );
            }
            "--tail" => {
                i += 1;
                let n: i64 = rest
                    .get(i)
                    .ok_or_else(|| AppError::Parse("expected --tail value".into()))?
                    .parse()
                    .map_err(|_| AppError::Parse("--tail must be an integer".into()))?;
                tail = Some(n);
            }
            "-f" | "--follow" => follow = true,
            other if !other.starts_with('-') && pod.is_none() => {
                pod = Some(other.to_string());
            }
            other => return Err(AppError::Parse(format!("unexpected token '{}'", other))),
        }
        i += 1;
    }
    Ok(Command::Logs {
        pod: pod.ok_or_else(|| AppError::Parse("pod name required".into()))?,
        container,
        tail,
        follow,
    })
}

fn parse_delete(rest: &[&str]) -> AppResult<Command> {
    if rest.len() < 2 {
        return Err(AppError::Parse("usage: delete <resource> <name>".into()));
    }
    Ok(Command::Delete {
        resource: Resource::parse(rest[0])?,
        name: rest[1].to_string(),
    })
}

fn parse_scale(rest: &[&str]) -> AppResult<Command> {
    // forms accepted:
    //   scale deploy <name> --replicas N
    //   scale deploy/<name> --replicas N
    if rest.is_empty() {
        return Err(AppError::Parse(
            "usage: scale <resource> <name> --replicas N".into(),
        ));
    }
    let (resource, name, tail) = if let Some((r, n)) = rest[0].split_once('/') {
        (Resource::parse(r)?, n.to_string(), &rest[1..])
    } else if rest.len() >= 2 {
        (Resource::parse(rest[0])?, rest[1].to_string(), &rest[2..])
    } else {
        return Err(AppError::Parse(
            "usage: scale <resource> <name> --replicas N".into(),
        ));
    };

    let mut replicas: Option<i32> = None;
    let mut i = 0;
    while i < tail.len() {
        match tail[i] {
            "--replicas" => {
                i += 1;
                replicas = Some(
                    tail.get(i)
                        .ok_or_else(|| AppError::Parse("expected replicas value".into()))?
                        .parse()
                        .map_err(|_| AppError::Parse("--replicas must be an integer".into()))?,
                );
            }
            other if other.starts_with("--replicas=") => {
                let v = &other["--replicas=".len()..];
                replicas = Some(
                    v.parse()
                        .map_err(|_| AppError::Parse("--replicas must be an integer".into()))?,
                );
            }
            other => return Err(AppError::Parse(format!("unexpected token '{}'", other))),
        }
        i += 1;
    }

    let replicas = replicas.ok_or_else(|| AppError::Parse("--replicas required".into()))?;
    if replicas < 0 {
        return Err(AppError::BadInput("--replicas must be >= 0".into()));
    }
    Ok(Command::Scale {
        resource,
        name,
        replicas,
    })
}

fn parse_rollout(rest: &[&str]) -> AppResult<Command> {
    // rollout restart <resource> <name>
    if rest.len() < 3 || rest[0].to_ascii_lowercase() != "restart" {
        return Err(AppError::Parse(
            "usage: rollout restart <resource> <name>".into(),
        ));
    }
    Ok(Command::RolloutRestart {
        resource: Resource::parse(rest[1])?,
        name: rest[2].to_string(),
    })
}

fn parse_apply(rest: &[&str]) -> AppResult<Command> {
    // apply paste:<id>   (the inline YAML is supplied via a separate field
    // and stashed under that id by the frontend before invoking).
    if rest.len() != 1 {
        return Err(AppError::Parse(
            "usage: apply paste:<id>  (paste YAML in the apply dialog first)".into(),
        ));
    }
    let token = rest[0];
    let id = token
        .strip_prefix("paste:")
        .ok_or_else(|| AppError::Parse("apply requires a paste:<id> reference".into()))?;
    if id.is_empty() {
        return Err(AppError::Parse("paste id is empty".into()));
    }
    Ok(Command::Apply {
        paste_id: id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_kubectl_prefix() {
        assert!(matches!(
            parse("kubectl get pods"),
            Err(AppError::Forbidden(_))
        ));
    }

    #[test]
    fn rejects_context_override() {
        assert!(matches!(
            parse("get pods --context other"),
            Err(AppError::Forbidden(_))
        ));
        assert!(matches!(
            parse("get pods --kubeconfig /tmp/kc"),
            Err(AppError::Forbidden(_))
        ));
        assert!(matches!(
            parse("get pods --server https://evil"),
            Err(AppError::Forbidden(_))
        ));
    }

    #[test]
    fn rejects_exec_and_portforward() {
        assert!(matches!(parse("exec my-pod -- sh"), Err(AppError::Forbidden(_))));
        assert!(matches!(parse("port-forward svc/x 8080"), Err(AppError::Forbidden(_))));
    }

    #[test]
    fn parses_get() {
        assert_eq!(
            parse("get pods").unwrap(),
            Command::Get {
                resource: Resource::Pods,
                name: None
            }
        );
        assert_eq!(
            parse("get deploy my-app").unwrap(),
            Command::Get {
                resource: Resource::Deployments,
                name: Some("my-app".into())
            }
        );
    }

    #[test]
    fn parses_logs_with_flags() {
        let c = parse("logs my-pod -c main --tail 50 -f").unwrap();
        assert_eq!(
            c,
            Command::Logs {
                pod: "my-pod".into(),
                container: Some("main".into()),
                tail: Some(50),
                follow: true,
            }
        );
    }

    #[test]
    fn parses_scale_slash_form() {
        let c = parse("scale deploy/my-app --replicas 3").unwrap();
        assert_eq!(
            c,
            Command::Scale {
                resource: Resource::Deployments,
                name: "my-app".into(),
                replicas: 3
            }
        );
    }

    #[test]
    fn parses_rollout_restart() {
        let c = parse("rollout restart deploy my-app").unwrap();
        assert_eq!(
            c,
            Command::RolloutRestart {
                resource: Resource::Deployments,
                name: "my-app".into()
            }
        );
    }

    #[test]
    fn parses_apply_paste_id() {
        let c = parse("apply paste:abc-123").unwrap();
        assert_eq!(c, Command::Apply { paste_id: "abc-123".into() });
    }

    #[test]
    fn empty_rejected() {
        assert!(parse("   ").is_err());
    }
}
