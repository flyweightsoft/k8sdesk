//! Maps parsed DSL commands to `kube-rs` API calls.
//! The cluster + namespace are passed in by the caller and **cannot** be
//! overridden by the user input (the parser already rejects override flags).

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{
    ConfigMap, Event, Namespace, Node, Pod, Secret, Service,
};
use k8s_openapi::api::networking::v1::Ingress;
use kube::{
    api::{Api, DeleteParams, ListParams, LogParams, Patch, PatchParams},
    core::DynamicObject,
    discovery::Scope,
    Client,
};
use serde::{Deserialize, Serialize};

use crate::dsl::parser::{Command, Resource};
use crate::error::{AppError, AppResult};

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandOutput {
    Table { headers: Vec<String>, rows: Vec<Vec<String>> },
    Yaml { body: String },
    Text { body: String },
    Ok { message: String },
}

pub async fn execute(
    client: Client,
    namespace: &str,
    cmd: Command,
    apply_body: Option<String>,
) -> AppResult<CommandOutput> {
    match cmd {
        Command::Help => Ok(CommandOutput::Text {
            body: HELP_TEXT.to_string(),
        }),
        Command::Get { resource, name } => exec_get(client, namespace, resource, name).await,
        Command::Describe { resource, name } => exec_describe(client, namespace, resource, &name).await,
        Command::Logs {
            pod,
            container,
            tail,
            follow,
        } => exec_logs(client, namespace, &pod, container, tail, follow).await,
        Command::Delete { resource, name } => exec_delete(client, namespace, resource, &name).await,
        Command::Scale {
            resource,
            name,
            replicas,
        } => exec_scale(client, namespace, resource, &name, replicas).await,
        Command::RolloutRestart { resource, name } => {
            exec_rollout_restart(client, namespace, resource, &name).await
        }
        Command::Apply { paste_id: _ } => {
            let body = apply_body.ok_or_else(|| {
                AppError::BadInput("apply requires a YAML body".into())
            })?;
            exec_apply(client, namespace, &body).await
        }
    }
}

const HELP_TEXT: &str = r#"Available commands (cluster + namespace are auto-applied):

  get <resource> [name]        list or fetch a resource
  describe <resource> <name>   full YAML for one resource
  logs <pod> [-c c] [--tail N] [-f]
  delete <resource> <name>     [destructive]
  scale <res> <name> --replicas N    [destructive]
  rollout restart <res> <name>       [destructive]
  apply paste:<id>             [destructive] (paste YAML first)
  help                         this message

Resources: pods, deploy, svc, nodes, ns, cm, secrets, events, rs, sts, ds, ing, jobs, cj
"#;

// ---------- get ----------

async fn exec_get(
    client: Client,
    ns: &str,
    res: Resource,
    name: Option<String>,
) -> AppResult<CommandOutput> {
    macro_rules! list_ns {
        ($t:ty, $hdrs:expr, $row:expr) => {{
            if ns == "*" {
                let api: Api<$t> = Api::all(client.clone());
                if let Some(n) = name {
                    let obj = api.get(&n).await?;
                    Ok(CommandOutput::Yaml {
                        body: serde_yaml::to_string(&obj).unwrap_or_default(),
                    })
                } else {
                    let list = api.list(&ListParams::default()).await?;
                    let mut headers = vec!["NAMESPACE".to_string()];
                    headers.extend($hdrs.iter().map(|s: &&str| s.to_string()));
                    let row_fn = $row;
                    let rows: Vec<Vec<String>> = list.items.iter().map(|item| {
                        let item_ns = item.metadata.namespace.clone().unwrap_or_default();
                        let mut row = vec![item_ns];
                        row.extend(row_fn(item));
                        row
                    }).collect();
                    Ok(CommandOutput::Table { headers, rows })
                }
            } else {
                let api: Api<$t> = Api::namespaced(client.clone(), ns);
                if let Some(n) = name {
                    let obj = api.get(&n).await?;
                    Ok(CommandOutput::Yaml {
                        body: serde_yaml::to_string(&obj).unwrap_or_default(),
                    })
                } else {
                    let list = api.list(&ListParams::default()).await?;
                    let rows: Vec<Vec<String>> = list.items.iter().map($row).collect();
                    Ok(CommandOutput::Table {
                        headers: $hdrs.iter().map(|s: &&str| s.to_string()).collect(),
                        rows,
                    })
                }
            }
        }};
    }

    macro_rules! list_cluster {
        ($t:ty, $hdrs:expr, $row:expr) => {{
            let api: Api<$t> = Api::all(client.clone());
            if let Some(n) = name {
                let obj = api.get(&n).await?;
                Ok(CommandOutput::Yaml {
                    body: serde_yaml::to_string(&obj).unwrap_or_default(),
                })
            } else {
                let list = api.list(&ListParams::default()).await?;
                let rows: Vec<Vec<String>> = list.items.iter().map($row).collect();
                Ok(CommandOutput::Table {
                    headers: $hdrs.iter().map(|s: &&str| s.to_string()).collect(),
                    rows,
                })
            }
        }};
    }

    match res {
        Resource::Pods => list_ns!(Pod, ["NAME", "PHASE", "NODE"], |p: &Pod| vec![
            p.metadata.name.clone().unwrap_or_default(),
            p.status
                .as_ref()
                .and_then(|s| s.phase.clone())
                .unwrap_or_default(),
            p.spec
                .as_ref()
                .and_then(|s| s.node_name.clone())
                .unwrap_or_default(),
        ]),
        Resource::Deployments => list_ns!(
            Deployment,
            ["NAME", "READY", "AVAILABLE"],
            |d: &Deployment| vec![
                d.metadata.name.clone().unwrap_or_default(),
                d.status
                    .as_ref()
                    .map(|s| format!(
                        "{}/{}",
                        s.ready_replicas.unwrap_or(0),
                        s.replicas.unwrap_or(0)
                    ))
                    .unwrap_or_default(),
                d.status
                    .as_ref()
                    .and_then(|s| s.available_replicas)
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
            ]
        ),
        Resource::Services => list_ns!(Service, ["NAME", "TYPE", "CLUSTER-IP"], |s: &Service| {
            vec![
                s.metadata.name.clone().unwrap_or_default(),
                s.spec
                    .as_ref()
                    .and_then(|x| x.type_.clone())
                    .unwrap_or_default(),
                s.spec
                    .as_ref()
                    .and_then(|x| x.cluster_ip.clone())
                    .unwrap_or_default(),
            ]
        }),
        Resource::ConfigMaps => list_ns!(ConfigMap, ["NAME", "DATA"], |c: &ConfigMap| vec![
            c.metadata.name.clone().unwrap_or_default(),
            c.data.as_ref().map(|d| d.len().to_string()).unwrap_or("0".into()),
        ]),
        Resource::Secrets => list_ns!(Secret, ["NAME", "TYPE"], |s: &Secret| vec![
            s.metadata.name.clone().unwrap_or_default(),
            s.type_.clone().unwrap_or_default(),
        ]),
        Resource::Events => list_ns!(Event, ["LAST", "TYPE", "REASON", "MESSAGE"], |e: &Event| {
            vec![
                e.last_timestamp
                    .as_ref()
                    .map(|t| t.0.to_rfc3339())
                    .unwrap_or_default(),
                e.type_.clone().unwrap_or_default(),
                e.reason.clone().unwrap_or_default(),
                e.message.clone().unwrap_or_default(),
            ]
        }),
        Resource::ReplicaSets => list_ns!(ReplicaSet, ["NAME", "DESIRED", "READY"], |r: &ReplicaSet| vec![
            r.metadata.name.clone().unwrap_or_default(),
            r.spec.as_ref().and_then(|s| s.replicas).map(|n| n.to_string()).unwrap_or_default(),
            r.status.as_ref().and_then(|s| s.ready_replicas).map(|n| n.to_string()).unwrap_or_default(),
        ]),
        Resource::StatefulSets => list_ns!(StatefulSet, ["NAME", "READY"], |s: &StatefulSet| vec![
            s.metadata.name.clone().unwrap_or_default(),
            s.status.as_ref().map(|x| format!("{}/{}", x.ready_replicas.unwrap_or(0), x.replicas)).unwrap_or_default(),
        ]),
        Resource::DaemonSets => list_ns!(DaemonSet, ["NAME", "READY", "AVAILABLE"], |d: &DaemonSet| vec![
            d.metadata.name.clone().unwrap_or_default(),
            d.status.as_ref().map(|s| s.number_ready.to_string()).unwrap_or_default(),
            d.status.as_ref().and_then(|s| s.number_available).map(|n| n.to_string()).unwrap_or_default(),
        ]),
        Resource::Ingresses => list_ns!(Ingress, ["NAME", "CLASS"], |i: &Ingress| vec![
            i.metadata.name.clone().unwrap_or_default(),
            i.spec.as_ref().and_then(|s| s.ingress_class_name.clone()).unwrap_or_default(),
        ]),
        Resource::Jobs => list_ns!(Job, ["NAME", "COMPLETIONS"], |j: &Job| vec![
            j.metadata.name.clone().unwrap_or_default(),
            j.status.as_ref().and_then(|s| s.succeeded).map(|n| n.to_string()).unwrap_or_default(),
        ]),
        Resource::CronJobs => list_ns!(CronJob, ["NAME", "SCHEDULE"], |c: &CronJob| vec![
            c.metadata.name.clone().unwrap_or_default(),
            c.spec.as_ref().map(|s| s.schedule.clone()).unwrap_or_default(),
        ]),
        Resource::Nodes => list_cluster!(Node, ["NAME", "READY"], |n: &Node| {
            let ready = n
                .status
                .as_ref()
                .and_then(|s| s.conditions.as_ref())
                .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"))
                .map(|c| c.status.clone())
                .unwrap_or_default();
            vec![n.metadata.name.clone().unwrap_or_default(), ready]
        }),
        Resource::Namespaces => list_cluster!(Namespace, ["NAME", "PHASE"], |n: &Namespace| vec![
            n.metadata.name.clone().unwrap_or_default(),
            n.status
                .as_ref()
                .and_then(|s| s.phase.clone())
                .unwrap_or_default(),
        ]),
    }
}

// ---------- describe ----------

async fn exec_describe(
    client: Client,
    ns: &str,
    res: Resource,
    name: &str,
) -> AppResult<CommandOutput> {
    if ns == "*" {
        return Err(AppError::BadInput(
            "select a specific namespace — describe requires a single namespace".into(),
        ));
    }
    macro_rules! desc_ns {
        ($t:ty) => {{
            let api: Api<$t> = Api::namespaced(client.clone(), ns);
            let obj = api.get(name).await?;
            Ok(CommandOutput::Yaml {
                body: serde_yaml::to_string(&obj).unwrap_or_default(),
            })
        }};
    }
    macro_rules! desc_cluster {
        ($t:ty) => {{
            let api: Api<$t> = Api::all(client.clone());
            let obj = api.get(name).await?;
            Ok(CommandOutput::Yaml {
                body: serde_yaml::to_string(&obj).unwrap_or_default(),
            })
        }};
    }
    match res {
        Resource::Pods => desc_ns!(Pod),
        Resource::Deployments => desc_ns!(Deployment),
        Resource::Services => desc_ns!(Service),
        Resource::ConfigMaps => desc_ns!(ConfigMap),
        Resource::Secrets => desc_ns!(Secret),
        Resource::Events => desc_ns!(Event),
        Resource::ReplicaSets => desc_ns!(ReplicaSet),
        Resource::StatefulSets => desc_ns!(StatefulSet),
        Resource::DaemonSets => desc_ns!(DaemonSet),
        Resource::Ingresses => desc_ns!(Ingress),
        Resource::Jobs => desc_ns!(Job),
        Resource::CronJobs => desc_ns!(CronJob),
        Resource::Nodes => desc_cluster!(Node),
        Resource::Namespaces => desc_cluster!(Namespace),
    }
}

// ---------- logs ----------

async fn exec_logs(
    client: Client,
    ns: &str,
    pod: &str,
    container: Option<String>,
    tail: Option<i64>,
    _follow: bool, // streaming logs is intentionally out of MVP
) -> AppResult<CommandOutput> {
    if ns == "*" {
        return Err(AppError::BadInput(
            "select a specific namespace — logs requires a single namespace".into(),
        ));
    }
    let api: Api<Pod> = Api::namespaced(client, ns);
    let lp = LogParams {
        container,
        tail_lines: tail,
        timestamps: true,
        ..Default::default()
    };
    let body = api.logs(pod, &lp).await?;
    Ok(CommandOutput::Text { body })
}

// ---------- delete ----------

async fn exec_delete(
    client: Client,
    ns: &str,
    res: Resource,
    name: &str,
) -> AppResult<CommandOutput> {
    if ns == "*" {
        return Err(AppError::BadInput(
            "select a specific namespace — delete requires a single namespace".into(),
        ));
    }
    let dp = DeleteParams::default();
    macro_rules! del_ns {
        ($t:ty) => {{
            let api: Api<$t> = Api::namespaced(client.clone(), ns);
            api.delete(name, &dp).await?;
        }};
    }
    macro_rules! del_cluster {
        ($t:ty) => {{
            let api: Api<$t> = Api::all(client.clone());
            api.delete(name, &dp).await?;
        }};
    }
    match res {
        Resource::Pods => del_ns!(Pod),
        Resource::Deployments => del_ns!(Deployment),
        Resource::Services => del_ns!(Service),
        Resource::ConfigMaps => del_ns!(ConfigMap),
        Resource::Secrets => del_ns!(Secret),
        Resource::Events => del_ns!(Event),
        Resource::ReplicaSets => del_ns!(ReplicaSet),
        Resource::StatefulSets => del_ns!(StatefulSet),
        Resource::DaemonSets => del_ns!(DaemonSet),
        Resource::Ingresses => del_ns!(Ingress),
        Resource::Jobs => del_ns!(Job),
        Resource::CronJobs => del_ns!(CronJob),
        Resource::Nodes => del_cluster!(Node),
        Resource::Namespaces => del_cluster!(Namespace),
    }
    Ok(CommandOutput::Ok {
        message: format!("deleted {}", name),
    })
}

// ---------- scale ----------

async fn exec_scale(
    client: Client,
    ns: &str,
    res: Resource,
    name: &str,
    replicas: i32,
) -> AppResult<CommandOutput> {
    if ns == "*" {
        return Err(AppError::BadInput(
            "select a specific namespace — scale requires a single namespace".into(),
        ));
    }
    let patch = serde_json::json!({ "spec": { "replicas": replicas } });
    let pp = PatchParams::apply("k8sdesk").force();
    match res {
        Resource::Deployments => {
            let api: Api<Deployment> = Api::namespaced(client, ns);
            api.patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        Resource::StatefulSets => {
            let api: Api<StatefulSet> = Api::namespaced(client, ns);
            api.patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        Resource::ReplicaSets => {
            let api: Api<ReplicaSet> = Api::namespaced(client, ns);
            api.patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        other => {
            return Err(AppError::BadInput(format!(
                "scale not supported for resource {:?}",
                other
            )))
        }
    }
    Ok(CommandOutput::Ok {
        message: format!("scaled {} to {} replicas", name, replicas),
    })
}

// ---------- rollout restart ----------

async fn exec_rollout_restart(
    client: Client,
    ns: &str,
    res: Resource,
    name: &str,
) -> AppResult<CommandOutput> {
    if ns == "*" {
        return Err(AppError::BadInput(
            "select a specific namespace — rollout restart requires a single namespace".into(),
        ));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let patch = serde_json::json!({
        "spec": {
            "template": {
                "metadata": {
                    "annotations": {
                        "k8sdesk.dev/restartedAt": now
                    }
                }
            }
        }
    });
    let pp = PatchParams::default();
    match res {
        Resource::Deployments => {
            let api: Api<Deployment> = Api::namespaced(client, ns);
            api.patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        Resource::StatefulSets => {
            let api: Api<StatefulSet> = Api::namespaced(client, ns);
            api.patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        Resource::DaemonSets => {
            let api: Api<DaemonSet> = Api::namespaced(client, ns);
            api.patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        other => {
            return Err(AppError::BadInput(format!(
                "rollout restart not supported for {:?}",
                other
            )))
        }
    }
    Ok(CommandOutput::Ok {
        message: format!("rollout restart triggered for {}", name),
    })
}

// ---------- apply ----------

async fn exec_apply(client: Client, ns: &str, yaml: &str) -> AppResult<CommandOutput> {
    if ns == "*" {
        return Err(AppError::BadInput(
            "select a specific namespace — apply requires a single namespace".into(),
        ));
    }
    let docs: Vec<serde_yaml::Value> = serde_yaml::Deserializer::from_str(yaml)
        .map(serde_yaml::Value::deserialize)
        .collect::<Result<_, _>>()?;

    let mut applied = Vec::new();

    for doc in docs {
        if doc.is_null() {
            continue;
        }
        let mut obj: DynamicObject = serde_yaml::from_value(doc)?;

        // Force the configured namespace; never trust YAML metadata.namespace.
        obj.metadata.namespace = Some(ns.to_string());

        let tm = obj
            .types
            .clone()
            .ok_or_else(|| AppError::BadInput("apply: missing apiVersion/kind".into()))?;

        let (group, version) = match tm.api_version.split_once('/') {
            Some((g, v)) => (g.to_string(), v.to_string()),
            None => (String::new(), tm.api_version.clone()),
        };
        let gvk = kube::core::GroupVersionKind {
            group,
            version,
            kind: tm.kind.clone(),
        };

        let discovery = kube::Discovery::new(client.clone())
            .run()
            .await
            .map_err(|e| AppError::Kube(e.to_string()))?;
        let (ar, caps) = discovery
            .resolve_gvk(&gvk)
            .ok_or_else(|| AppError::BadInput(format!("unknown kind {}", tm.kind)))?;

        let api: Api<DynamicObject> = match caps.scope {
            Scope::Namespaced => Api::namespaced_with(client.clone(), ns, &ar),
            Scope::Cluster => Api::all_with(client.clone(), &ar),
        };

        let name = obj
            .metadata
            .name
            .clone()
            .ok_or_else(|| AppError::BadInput("apply: object missing metadata.name".into()))?;
        let pp = PatchParams::apply("k8sdesk").force();
        api.patch(&name, &pp, &Patch::Apply(&obj)).await?;
        applied.push(format!("{}/{}", tm.kind.to_ascii_lowercase(), name));
    }

    Ok(CommandOutput::Ok {
        message: format!("applied: {}", applied.join(", ")),
    })
}
