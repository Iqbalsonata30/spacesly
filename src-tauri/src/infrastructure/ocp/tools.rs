use serde_json::{json, Value};

use super::client::OcpClient;
use super::errors::{OcpError, OcpResult};

pub const MAX_LIST_ITEMS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcpTool {
    GetNamespaces,
    GetNodes,
    GetPods,
    GetPod,
    GetDeployments,
    GetStatefulSets,
    GetDaemonSets,
    GetServices,
    GetDeploymentConfigs,
    GetRollouts,
    GetEvents,
    PodLogs,
    RestartDeployment,
    ScaleDeployment,
    DeleteManagedPod,
}

impl OcpTool {
    pub fn all() -> &'static [OcpTool] {
        &[
            Self::GetNamespaces,
            Self::GetNodes,
            Self::GetPods,
            Self::GetPod,
            Self::GetDeployments,
            Self::GetStatefulSets,
            Self::GetDaemonSets,
            Self::GetServices,
            Self::GetDeploymentConfigs,
            Self::GetRollouts,
            Self::GetEvents,
            Self::PodLogs,
            Self::RestartDeployment,
            Self::ScaleDeployment,
            Self::DeleteManagedPod,
        ]
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|tool| tool.as_str() == name)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GetNamespaces => "ocp_get_namespaces",
            Self::GetNodes => "ocp_get_nodes",
            Self::GetPods => "ocp_get_pods",
            Self::GetPod => "ocp_get_pod",
            Self::GetDeployments => "ocp_get_deployments",
            Self::GetStatefulSets => "ocp_get_statefulsets",
            Self::GetDaemonSets => "ocp_get_daemonsets",
            Self::GetServices => "ocp_get_services",
            Self::GetDeploymentConfigs => "ocp_get_deploymentconfigs",
            Self::GetRollouts => "ocp_get_rollouts",
            Self::GetEvents => "ocp_get_events",
            Self::PodLogs => "ocp_pod_logs",
            Self::RestartDeployment => "ocp_restart_deployment",
            Self::ScaleDeployment => "ocp_scale_deployment",
            Self::DeleteManagedPod => "ocp_delete_managed_pod",
        }
    }

    pub fn is_mutation(self) -> bool {
        matches!(
            self,
            Self::RestartDeployment | Self::ScaleDeployment | Self::DeleteManagedPod
        )
    }

    pub fn spec(self) -> Value {
        let namespace_prop = json!({
            "type": "string",
            "description": "Namespace to query. Defaults to the configured namespace."
        });
        let name_prop = json!({
            "type": "string",
            "description": "Resource name."
        });
        let properties = match self {
            Self::GetNamespaces | Self::GetNodes => json!({}),
            Self::GetPods
            | Self::GetDeployments
            | Self::GetStatefulSets
            | Self::GetDaemonSets
            | Self::GetServices
            | Self::GetDeploymentConfigs
            | Self::GetRollouts
            | Self::GetEvents => json!({
                "namespace": namespace_prop,
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "description": "Maximum number of items to return."
                }
            }),
            Self::GetPod => json!({
                "name": name_prop,
                "namespace": namespace_prop
            }),
            Self::RestartDeployment => json!({
                "name": name_prop,
                "namespace": namespace_prop
            }),
            Self::ScaleDeployment => json!({
                "name": name_prop,
                "namespace": namespace_prop,
                "replicas": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 1000,
                    "description": "Desired replica count. Scaling to zero stops the workload."
                }
            }),
            Self::DeleteManagedPod => json!({
                "name": name_prop,
                "namespace": namespace_prop
            }),
            Self::PodLogs => json!({
                "pod": name_prop,
                "namespace": namespace_prop,
                "container": {
                    "type": "string",
                    "description": "Container name within the pod. Defaults to the first container."
                },
                "tail_lines": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 5000,
                    "description": "Number of recent log lines to return."
                }
            }),
        };
        json!({
            "name": self.as_str(),
            "description": self.description(),
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "additionalProperties": false
            }
        })
    }

    fn description(self) -> &'static str {
        match self {
            Self::GetNamespaces => "List namespaces visible to the configured cluster identity.",
            Self::GetNodes => "List cluster nodes and their status.",
            Self::GetPods => "List pods in a namespace with their status.",
            Self::GetPod => "Get one pod and its current status.",
            Self::GetDeployments => "List Deployments and their replica status.",
            Self::GetStatefulSets => "List StatefulSets and their replica status.",
            Self::GetDaemonSets => "List DaemonSets and their desired/ready counts.",
            Self::GetServices => "List Services in a namespace.",
            Self::GetDeploymentConfigs => {
                "List OpenShift DeploymentConfigs and their replica status."
            }
            Self::GetRollouts => "List Argo Rollouts and their rollout status.",
            Self::GetEvents => "List recent events in a namespace, newest first.",
            Self::PodLogs => "Fetch recent log output for a pod container.",
            Self::RestartDeployment => {
                "Restart a Deployment by updating its pod-template annotation. Requires explicit operator approval in Spacesly."
            }
            Self::ScaleDeployment => {
                "Scale a Deployment to an explicit replica count. Requires explicit operator approval in Spacesly."
            }
            Self::DeleteManagedPod => {
                "Delete one controller-managed Pod so Kubernetes can replace it. Standalone Pods are refused. Requires explicit operator approval in Spacesly."
            }
        }
    }
}

pub fn tool_metadata() -> Vec<Value> {
    OcpTool::all().iter().map(|tool| tool.spec()).collect()
}

pub fn execute_tool(client: &OcpClient, name: &str, arguments: &Value) -> OcpResult<Value> {
    let tool = OcpTool::parse(name)
        .ok_or_else(|| OcpError::internal(format!("Unknown OCP tool '{name}'.")))?;
    if !arguments.is_object() {
        return Err(OcpError::config(
            "invalid_arguments",
            "OCP tool arguments must be a JSON object.",
        ));
    }
    match tool {
        OcpTool::GetNamespaces => {
            let items = client.get_namespaces()?;
            Ok(json!({ "kind": "NamespaceList", "items": items }))
        }
        OcpTool::GetNodes => {
            let items = client.get_list("/api/v1/nodes", &[], false)?;
            Ok(summarized_list("NodeList", &items, None))
        }
        OcpTool::GetPods => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items = client.list_namespaced("", "v1", "pods", namespace.as_deref())?;
            Ok(summarized_list("PodList", &items, namespace.as_deref()).with_limit(limit))
        }
        OcpTool::GetPod => {
            let namespace = namespace_argument(client, arguments)?;
            let name = required_string(arguments, "name")?;
            let item = client.get_namespaced("", "v1", "pods", &name, namespace.as_deref())?;
            let mut response = json!({ "kind": "Pod", "item": summarize_item(&item) });
            if let Some(namespace) = namespace {
                response["namespace"] = json!(namespace);
            }
            Ok(response)
        }
        OcpTool::GetDeployments => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items =
                client.list_namespaced("apps", "v1", "deployments", namespace.as_deref())?;
            Ok(summarized_list("DeploymentList", &items, namespace.as_deref()).with_limit(limit))
        }
        OcpTool::GetStatefulSets => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items =
                client.list_namespaced("apps", "v1", "statefulsets", namespace.as_deref())?;
            Ok(summarized_list("StatefulSetList", &items, namespace.as_deref()).with_limit(limit))
        }
        OcpTool::GetDaemonSets => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items = client.list_namespaced("apps", "v1", "daemonsets", namespace.as_deref())?;
            Ok(summarized_list("DaemonSetList", &items, namespace.as_deref()).with_limit(limit))
        }
        OcpTool::GetServices => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items = client.list_namespaced("", "v1", "services", namespace.as_deref())?;
            Ok(summarized_list("ServiceList", &items, namespace.as_deref()).with_limit(limit))
        }
        OcpTool::GetDeploymentConfigs => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items = client.list_namespaced(
                "apps.openshift.io",
                "v1",
                "deploymentconfigs",
                namespace.as_deref(),
            )?;
            Ok(
                summarized_list("DeploymentConfigList", &items, namespace.as_deref())
                    .with_limit(limit),
            )
        }
        OcpTool::GetRollouts => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items = client.list_namespaced(
                "argoproj.io",
                "v1alpha1",
                "rollouts",
                namespace.as_deref(),
            )?;
            Ok(summarized_list("RolloutList", &items, namespace.as_deref()).with_limit(limit))
        }
        OcpTool::GetEvents => {
            let namespace = namespace_argument(client, arguments)?;
            let limit = limit_argument(arguments)?;
            let items = client.get_list(
                &format!(
                    "/api/v1/namespaces/{}/events",
                    namespace
                        .as_deref()
                        .unwrap_or_else(|| client.default_namespace())
                ),
                &[],
                false,
            )?;
            Ok(summarized_list("EventList", &items, namespace.as_deref()).with_limit(limit))
        }
        OcpTool::PodLogs => {
            let namespace = namespace_argument(client, arguments)?;
            let pod = required_string(arguments, "pod")?;
            let container = optional_string(arguments, "container");
            let tail_lines = optional_u32(arguments, "tail_lines");
            let logs = client.pod_logs(
                namespace
                    .as_deref()
                    .unwrap_or_else(|| client.default_namespace()),
                &pod,
                container.as_deref(),
                tail_lines,
            )?;
            Ok(json!({ "kind": "Log", "pod": pod, "logs": logs }))
        }
        OcpTool::RestartDeployment => {
            let namespace = namespace_argument(client, arguments)?;
            let name = required_string(arguments, "name")?;
            let restarted_at = chrono::Utc::now().to_rfc3339();
            let item = client.patch_namespaced(
                "apps",
                "v1",
                "deployments",
                &name,
                namespace.as_deref(),
                &json!({
                    "spec": {
                        "template": {
                            "metadata": {
                                "annotations": {
                                    "kubectl.kubernetes.io/restartedAt": restarted_at
                                }
                            }
                        }
                    }
                }),
            )?;
            Ok(json!({
                "kind": "DeploymentRestart",
                "name": name,
                "namespace": namespace,
                "generation": item["metadata"]["generation"],
                "restarted_at": restarted_at
            }))
        }
        OcpTool::ScaleDeployment => {
            let namespace = namespace_argument(client, arguments)?;
            let name = required_string(arguments, "name")?;
            let replicas = required_u32(arguments, "replicas", 1000)?;
            let item = client.patch_namespaced(
                "apps",
                "v1",
                "deployments",
                &name,
                namespace.as_deref(),
                &json!({ "spec": { "replicas": replicas } }),
            )?;
            Ok(json!({
                "kind": "DeploymentScale",
                "name": name,
                "namespace": namespace,
                "replicas": item["spec"]["replicas"],
                "generation": item["metadata"]["generation"]
            }))
        }
        OcpTool::DeleteManagedPod => {
            let namespace = namespace_argument(client, arguments)?;
            let name = required_string(arguments, "name")?;
            let pod = client.get_namespaced("", "v1", "pods", &name, namespace.as_deref())?;
            let owner = controller_owner(&pod).ok_or_else(|| {
                OcpError::config(
                    "standalone_pod_refused",
                    format!(
                        "Pod '{name}' has no controller owner. Spacesly refuses to delete standalone Pods."
                    ),
                )
            })?;
            client.delete_namespaced("", "v1", "pods", &name, namespace.as_deref())?;
            Ok(json!({
                "kind": "ManagedPodDeletion",
                "name": name,
                "namespace": namespace,
                "owner": owner,
                "status": "deletion_requested"
            }))
        }
    }
}

trait WithLimit {
    fn with_limit(self, limit: Option<u32>) -> Value;
}

impl WithLimit for Value {
    fn with_limit(mut self, limit: Option<u32>) -> Value {
        let Some(limit) = limit else {
            return self;
        };
        if let Some(items) = self.get_mut("items").and_then(|value| value.as_array_mut()) {
            let max = (limit as usize).min(MAX_LIST_ITEMS);
            items.truncate(max);
            self["truncated"] = json!(items.len() >= max);
        }
        self
    }
}

fn summarized_list(kind: &str, items: &Value, namespace: Option<&str>) -> Value {
    let summarized: Vec<Value> = items
        .as_array()
        .map(|items| items.iter().map(summarize_item).collect())
        .unwrap_or_default();
    let mut value = json!({
        "kind": kind,
        "count": summarized.len(),
        "items": summarized,
    });
    if let Some(namespace) = namespace {
        value["namespace"] = json!(namespace);
    }
    value
}

fn summarize_item(item: &Value) -> Value {
    let name = item["metadata"]["name"].as_str().unwrap_or("").to_string();
    let namespace = item["metadata"]["namespace"].as_str().map(str::to_string);
    let kind = item["kind"].as_str().unwrap_or("").to_string();
    let mut summary = json!({
        "name": name,
        "kind": kind,
    });
    if let Some(namespace) = namespace {
        summary["namespace"] = json!(namespace);
    }
    if let Some(created) = item["metadata"]["creationTimestamp"].as_str() {
        summary["created"] = json!(created);
    }
    summarize_status(item, &mut summary);
    summary
}

fn summarize_status(item: &Value, summary: &mut Value) {
    let status = &item["status"];
    if let Some(phase) = status["phase"].as_str() {
        summary["phase"] = json!(phase);
    }
    if let Some(desired) = item["spec"]["replicas"].as_u64() {
        summary["desired_replicas"] = json!(desired);
    }
    if let Some(ready) = status["readyReplicas"].as_u64() {
        summary["ready_replicas"] = json!(ready);
    }
    if let Some(replicas) = status["replicas"].as_u64() {
        summary["replicas"] = json!(replicas);
    }
    if let Some(image) = item["spec"]["template"]["spec"]["containers"]
        .as_array()
        .and_then(|containers| containers.first())
        .and_then(|container| container["image"].as_str())
    {
        summary["image"] = json!(image);
    }
    if let Some(node) = status["nodeName"].as_str() {
        summary["node"] = json!(node);
    }
    if let Some(ip) = status["podIP"].as_str() {
        summary["pod_ip"] = json!(ip);
    }
    if let Some(host_ip) = status["hostIP"].as_str() {
        summary["host_ip"] = json!(host_ip);
    }
    if let Some(reason) = status["reason"].as_str() {
        summary["reason"] = json!(reason);
    }
    if let Some(ready) = status["readyReplicas"].as_u64() {
        summary["ready"] = json!(ready);
    }
}

fn namespace_argument(client: &OcpClient, arguments: &Value) -> OcpResult<Option<String>> {
    let value = arguments
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(value) = &value {
        validate_identifier(value, "namespace")?;
    }
    Ok(value.or_else(|| Some(client.default_namespace().to_string())))
}

fn required_string(arguments: &Value, key: &str) -> OcpResult<String> {
    let value = arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            OcpError::config(
                "invalid_arguments",
                format!("OCP tool argument '{key}' is required."),
            )
        })?;
    validate_identifier(&value, key)?;
    Ok(value)
}

fn optional_string(arguments: &Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn optional_u32(arguments: &Value, key: &str) -> Option<u32> {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .map(|value| value as u32)
}

fn required_u32(arguments: &Value, key: &str, maximum: u32) -> OcpResult<u32> {
    let value = arguments.get(key).and_then(Value::as_u64).ok_or_else(|| {
        OcpError::config(
            "invalid_arguments",
            format!("OCP tool argument '{key}' is required."),
        )
    })?;
    if value > u64::from(maximum) {
        return Err(OcpError::config(
            "invalid_arguments",
            format!("OCP tool argument '{key}' must not exceed {maximum}."),
        ));
    }
    Ok(value as u32)
}

fn controller_owner(pod: &Value) -> Option<Value> {
    pod["metadata"]["ownerReferences"]
        .as_array()?
        .iter()
        .find(|owner| owner["controller"].as_bool() == Some(true))
        .map(|owner| {
            json!({
                "api_version": owner["apiVersion"],
                "kind": owner["kind"],
                "name": owner["name"]
            })
        })
}

fn limit_argument(arguments: &Value) -> OcpResult<Option<u32>> {
    match optional_u32(arguments, "limit") {
        Some(0) => Err(OcpError::config(
            "invalid_arguments",
            "OCP tool argument 'limit' must be positive.",
        )),
        value => Ok(value),
    }
}

fn validate_identifier(value: &str, field: &str) -> OcpResult<()> {
    if value.len() > 253
        || value.starts_with('-')
        || value.ends_with('-')
        || value
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.')))
    {
        return Err(OcpError::config(
            "invalid_arguments",
            format!("OCP tool argument '{field}' contains an invalid resource name."),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_tool_parses_round_trip() {
        for tool in OcpTool::all() {
            assert_eq!(OcpTool::parse(tool.as_str()), Some(*tool));
        }
        assert_eq!(OcpTool::parse("ocp_delete_pod"), None);
    }

    #[test]
    fn tool_specs_are_stable() {
        for spec in tool_metadata() {
            assert_eq!(spec["inputSchema"]["additionalProperties"], json!(false));
            assert!(!spec["description"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn mutation_tools_are_explicitly_classified() {
        let mutations = OcpTool::all()
            .iter()
            .filter(|tool| tool.is_mutation())
            .map(|tool| tool.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            mutations,
            vec![
                "ocp_restart_deployment",
                "ocp_scale_deployment",
                "ocp_delete_managed_pod"
            ]
        );
    }

    #[test]
    fn managed_pod_requires_a_controller_owner() {
        let pod = json!({
            "metadata": {
                "ownerReferences": [
                    { "apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "web-abc", "controller": true }
                ]
            }
        });
        assert_eq!(controller_owner(&pod).unwrap()["kind"], json!("ReplicaSet"));
        assert!(controller_owner(&json!({ "metadata": {} })).is_none());
    }

    #[test]
    fn rejects_missing_required_arguments() {
        let error = required_string(&json!({}), "pod").unwrap_err();
        assert!(error.message.contains("'pod' is required"));
    }

    #[test]
    fn rejects_invalid_namespace_identifiers() {
        assert!(validate_identifier("team-a", "namespace").is_ok());
        assert!(validate_identifier("../../etc", "namespace").is_err());
        assert!(validate_identifier("-bad", "namespace").is_err());
    }

    #[test]
    fn list_truncation_honors_limit() {
        let mut value = json!({ "items": [1, 2, 3, 4, 5] });
        value = value.with_limit(Some(2));
        assert_eq!(value["items"].as_array().unwrap().len(), 2);
        assert_eq!(value["truncated"], json!(true));
    }

    #[test]
    fn summarize_item_extracts_pod_status() {
        let item = json!({
            "kind": "Pod",
            "metadata": {"name": "web-0", "namespace": "team-a", "creationTimestamp": "2026-01-01T00:00:00Z"},
            "spec": {"template": {"spec": {"containers": [{"image": "nginx:1.25"}]}}},
            "status": {"phase": "Running", "podIP": "10.0.0.5", "nodeName": "node-1"}
        });
        let summary = summarize_item(&item);
        assert_eq!(summary["name"], json!("web-0"));
        assert_eq!(summary["namespace"], json!("team-a"));
        assert_eq!(summary["phase"], json!("Running"));
        assert_eq!(summary["image"], json!("nginx:1.25"));
        assert_eq!(summary["node"], json!("node-1"));
    }
}
