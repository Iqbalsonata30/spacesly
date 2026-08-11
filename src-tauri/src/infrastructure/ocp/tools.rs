use serde_json::{json, Value};

use crate::domain::resource_idempotency::{
    state_fingerprint, ResourceExecutionResult, ResourceExecutionStatus, ResourceIdentity,
    ResourceLookupResult, ResourceLookupStatus, ResourceMutationEvidence,
    ResourceOperationIdentity, ResourceRetryResumeStatus,
};

use super::client::{DiscoveredResource, KubernetesPatchType, OcpClient};
use super::errors::{OcpError, OcpErrorKind, OcpResult};

pub const MAX_LIST_ITEMS: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcpTool {
    KubernetesResourcesList,
    KubernetesResourcesGet,
    KubernetesResourcesCreate,
    KubernetesResourcesUpdate,
    KubernetesResourcesPatch,
    KubernetesResourcesDelete,
    KubernetesEventsList,
    KubernetesNamespacesList,
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
            Self::KubernetesResourcesList,
            Self::KubernetesResourcesGet,
            Self::KubernetesResourcesCreate,
            Self::KubernetesResourcesUpdate,
            Self::KubernetesResourcesPatch,
            Self::KubernetesResourcesDelete,
            Self::KubernetesEventsList,
            Self::KubernetesNamespacesList,
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
            Self::KubernetesResourcesList => "kubernetes_resources_list",
            Self::KubernetesResourcesGet => "kubernetes_resources_get",
            Self::KubernetesResourcesCreate => "kubernetes_resources_create",
            Self::KubernetesResourcesUpdate => "kubernetes_resources_update",
            Self::KubernetesResourcesPatch => "kubernetes_resources_patch",
            Self::KubernetesResourcesDelete => "kubernetes_resources_delete",
            Self::KubernetesEventsList => "kubernetes_events_list",
            Self::KubernetesNamespacesList => "kubernetes_namespaces_list",
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
            Self::KubernetesResourcesCreate
                | Self::KubernetesResourcesUpdate
                | Self::KubernetesResourcesPatch
                | Self::KubernetesResourcesDelete
                | Self::RestartDeployment
                | Self::ScaleDeployment
                | Self::DeleteManagedPod
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
        let api_version_prop = json!({
            "type": "string",
            "description": "Kubernetes apiVersion, for example v1, apps/v1, or example.io/v1alpha1."
        });
        let kind_prop = json!({
            "type": "string",
            "description": "Kubernetes Kind. Supply either kind or resource."
        });
        let resource_prop = json!({
            "type": "string",
            "description": "Discovered plural, singular, or short resource name. Supply either resource or kind."
        });
        let generic_identity = || {
            json!({
                "api_version": api_version_prop,
                "kind": kind_prop,
                "resource": resource_prop,
                "namespace": {
                    "type": "string",
                    "description": "Required for namespaced resources and forbidden for cluster-scoped resources."
                }
            })
        };
        let properties = match self {
            Self::KubernetesResourcesList => {
                let mut properties = generic_identity();
                properties["label_selector"] = json!({ "type": "string" });
                properties["field_selector"] = json!({ "type": "string" });
                properties["limit"] = json!({ "type": "integer", "minimum": 1, "maximum": 200 });
                properties["continue"] = json!({
                    "type": "string",
                    "description": "Opaque Kubernetes list continuation token from a previous response."
                });
                properties
            }
            Self::KubernetesResourcesGet | Self::KubernetesResourcesDelete => {
                let mut properties = generic_identity();
                properties["name"] = name_prop;
                if self == Self::KubernetesResourcesDelete {
                    properties["propagation_policy"] = json!({
                        "type": "string",
                        "enum": ["Foreground", "Background", "Orphan"]
                    });
                    properties["grace_period_seconds"] =
                        json!({ "type": "integer", "minimum": 0, "maximum": 86400 });
                    properties["dry_run"] = json!({ "type": "boolean" });
                }
                properties
            }
            Self::KubernetesResourcesCreate | Self::KubernetesResourcesUpdate => json!({
                "manifest": {
                    "type": "object",
                    "description": "Complete structured Kubernetes object including apiVersion, kind, metadata.name, and metadata.namespace when namespaced. Update also requires metadata.resourceVersion."
                }
            }),
            Self::KubernetesResourcesPatch => {
                let mut properties = generic_identity();
                properties["name"] = name_prop;
                properties["patch"] = json!({
                    "description": "Structured merge/apply object or RFC 6902 JSON Patch array."
                });
                properties["patch_type"] = json!({
                    "type": "string",
                    "enum": ["merge", "json", "server_apply"],
                    "description": "merge uses JSON Merge Patch; json uses RFC 6902; server_apply uses Kubernetes server-side apply."
                });
                properties["field_manager"] = json!({
                    "type": "string",
                    "description": "Required for server_apply."
                });
                properties["force"] = json!({
                    "type": "boolean",
                    "description": "Only applies to server_apply; request ownership takeover on conflicts."
                });
                properties
            }
            Self::KubernetesEventsList => json!({
                "namespace": {
                    "type": "string",
                    "description": "Namespace to query. Omit to list events across all namespaces."
                },
                "involved_object_name": { "type": "string" },
                "involved_object_kind": { "type": "string" },
                "reason": { "type": "string" },
                "type": { "type": "string", "enum": ["Normal", "Warning"] },
                "field_selector": { "type": "string" },
                "label_selector": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "continue": { "type": "string" }
            }),
            Self::KubernetesNamespacesList => json!({
                "label_selector": { "type": "string" },
                "field_selector": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "continue": { "type": "string" }
            }),
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
                "namespace": namespace_prop,
                "restart_token": {
                    "type": "string",
                    "description": "Globally unique lowercase UUIDv4 for this semantic restart objective. Reuse it on retries; use a new UUID for a later independent restart."
                }
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
        let mut input_schema = json!({
            "type": "object",
            "properties": properties,
            "required": self.required_properties(),
            "additionalProperties": false
        });
        if matches!(
            self,
            Self::KubernetesResourcesList
                | Self::KubernetesResourcesGet
                | Self::KubernetesResourcesPatch
                | Self::KubernetesResourcesDelete
        ) {
            input_schema["oneOf"] = json!([
                { "required": ["kind"] },
                { "required": ["resource"] }
            ]);
        }
        json!({
            "name": self.as_str(),
            "description": self.description(),
            "inputSchema": input_schema
        })
    }

    fn required_properties(self) -> Vec<&'static str> {
        match self {
            Self::KubernetesResourcesList => vec!["api_version"],
            Self::KubernetesResourcesGet | Self::KubernetesResourcesDelete => {
                vec!["api_version", "name"]
            }
            Self::KubernetesResourcesCreate | Self::KubernetesResourcesUpdate => vec!["manifest"],
            Self::KubernetesResourcesPatch => {
                vec!["api_version", "name", "patch", "patch_type"]
            }
            Self::RestartDeployment => vec!["name", "restart_token"],
            _ => Vec::new(),
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::KubernetesResourcesList => {
                "List arbitrary discovered Kubernetes resources with selectors and pagination."
            }
            Self::KubernetesResourcesGet => {
                "Get one arbitrary discovered Kubernetes resource as a structured object."
            }
            Self::KubernetesResourcesCreate => {
                "Create a structured Kubernetes manifest. Requires explicit operator approval in Spacesly."
            }
            Self::KubernetesResourcesUpdate => {
                "Replace a Kubernetes resource using an explicit resourceVersion. Requires explicit operator approval in Spacesly."
            }
            Self::KubernetesResourcesPatch => {
                "Patch a Kubernetes resource using merge patch, RFC 6902 JSON Patch, or server-side apply. Requires explicit operator approval in Spacesly."
            }
            Self::KubernetesResourcesDelete => {
                "Delete an arbitrary Kubernetes resource with explicit delete options. Destructive and requires explicit operator approval in Spacesly."
            }
            Self::KubernetesEventsList => {
                "List normalized Kubernetes events in one namespace or across all namespaces with server-side filters where supported."
            }
            Self::KubernetesNamespacesList => {
                "List namespaces with phase, labels, annotations, and creation timestamp."
            }
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
        OcpTool::KubernetesResourcesList => generic_resource_list(client, arguments),
        OcpTool::KubernetesResourcesGet => generic_resource_get(client, arguments),
        OcpTool::KubernetesResourcesCreate => generic_resource_create(client, arguments),
        OcpTool::KubernetesResourcesUpdate => generic_resource_update(client, arguments),
        OcpTool::KubernetesResourcesPatch => generic_resource_patch(client, arguments),
        OcpTool::KubernetesResourcesDelete => generic_resource_delete(client, arguments),
        OcpTool::KubernetesEventsList => kubernetes_events_list(client, arguments),
        OcpTool::KubernetesNamespacesList => kubernetes_namespaces_list(client, arguments),
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
        OcpTool::RestartDeployment => restart_deployment(client, arguments),
        OcpTool::ScaleDeployment => scale_deployment(client, arguments),
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

pub(super) fn resource_operation_identity(
    client: &OcpClient,
    name: &str,
    arguments: &Value,
) -> OcpResult<Option<ResourceOperationIdentity>> {
    match OcpTool::parse(name) {
        Some(OcpTool::RestartDeployment) => restart_resource_operation_identity(
            client.idempotency_environment(),
            client.default_namespace(),
            arguments,
        )
        .map(Some),
        Some(OcpTool::ScaleDeployment) => scale_resource_operation_identity(
            client.idempotency_environment(),
            client.default_namespace(),
            arguments,
        )
        .map(Some),
        _ => Ok(None),
    }
}

pub(super) fn restart_resource_operation_identity(
    environment: &str,
    default_namespace: &str,
    arguments: &Value,
) -> OcpResult<ResourceOperationIdentity> {
    let namespace = arguments
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_namespace)
        .to_string();
    validate_identifier(&namespace, "namespace")?;
    let name = required_string(arguments, "name")?;
    let restart_token = required_restart_token(arguments)?;
    ResourceOperationIdentity::new(
        "openshift_kubernetes",
        "restart_deployment",
        ResourceIdentity {
            api_version: "apps/v1".to_string(),
            kind: "Deployment".to_string(),
            namespace: Some(namespace),
            name,
        },
        environment,
        &json!({ "restart_token": restart_token }),
    )
    .map_err(|error| OcpError::internal(format!("Could not identify Deployment restart: {error}")))
}

pub(super) fn scale_resource_operation_identity(
    environment: &str,
    default_namespace: &str,
    arguments: &Value,
) -> OcpResult<ResourceOperationIdentity> {
    let namespace = arguments
        .get("namespace")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_namespace)
        .to_string();
    validate_identifier(&namespace, "namespace")?;
    let name = required_string(arguments, "name")?;
    let replicas = required_u32(arguments, "replicas", 1000)?;
    ResourceOperationIdentity::new(
        "openshift_kubernetes",
        "scale_deployment",
        ResourceIdentity {
            api_version: "apps/v1".to_string(),
            kind: "Deployment".to_string(),
            namespace: Some(namespace),
            name,
        },
        environment,
        &json!({ "replicas": replicas }),
    )
    .map_err(|error| OcpError::internal(format!("Could not identify Deployment scale: {error}")))
}

fn restart_deployment(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    const RESTART_ANNOTATION_POINTER: &str =
        "/spec/template/metadata/annotations/spacesly.dev~1restart-token";
    let identity =
        resource_operation_identity(client, OcpTool::RestartDeployment.as_str(), arguments)?
            .ok_or_else(|| OcpError::internal("Deployment restart identity was unavailable."))?;
    let namespace = identity
        .resource
        .namespace
        .clone()
        .ok_or_else(|| OcpError::internal("Deployment restart namespace was unavailable."))?;
    let name = identity.resource.name.clone();
    let _restart_token = required_restart_token(arguments)?;
    let desired_fingerprint = identity.mutation_fingerprint.clone();
    let current = client
        .get_namespaced("apps", "v1", "deployments", &name, Some(&namespace))
        .map_err(|error| {
            error.with_resource_mutation(ResourceMutationEvidence {
                identity: identity.clone(),
                lookup: ResourceLookupResult {
                    status: ResourceLookupStatus::Unavailable,
                    observed_fingerprint: None,
                    observed_version: None,
                },
                execution: ResourceExecutionResult {
                    status: ResourceExecutionStatus::Blocked,
                    resulting_fingerprint: None,
                    resulting_version: None,
                },
                retry_resume_status: ResourceRetryResumeStatus::AwaitingOperator,
            })
        })?;
    let current_marker = current
        .pointer(RESTART_ANNOTATION_POINTER)
        .and_then(Value::as_str);
    let observed_fingerprint = current_marker
        .map(|marker| {
            if marker.strip_prefix("sha256:").is_some_and(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) {
                Ok(marker.to_string())
            } else {
                state_fingerprint(&json!({ "restart_marker": marker })).map_err(|error| {
                    OcpError::internal(format!(
                        "Could not fingerprint Deployment restart state: {error}"
                    ))
                })
            }
        })
        .transpose()?;
    let resource_version = current
        .pointer("/metadata/resourceVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if current_marker == Some(desired_fingerprint.as_str()) {
        let evidence = restart_evidence(
            identity,
            ResourceLookupStatus::AlreadySatisfied,
            observed_fingerprint,
            resource_version.clone(),
            ResourceExecutionStatus::Skipped,
            Some(desired_fingerprint),
            resource_version,
            ResourceRetryResumeStatus::AlreadyComplete,
        )?;
        return Ok(json!({
            "kind": "DeploymentRestart",
            "name": name,
            "namespace": namespace,
            "outcome": "already_satisfied",
            "resource_mutation": evidence
        }));
    }
    let Some(resource_version) = resource_version else {
        let evidence = restart_evidence(
            identity,
            ResourceLookupStatus::Incompatible,
            observed_fingerprint,
            None,
            ResourceExecutionStatus::Conflict,
            None,
            None,
            ResourceRetryResumeStatus::Conflict,
        )?;
        return Err(OcpError::conflict(
            "incompatible_existing_resource",
            format!(
                "Deployment '{namespace}/{name}' has no resourceVersion; restart was not attempted."
            ),
        )
        .with_resource_mutation(evidence));
    };
    let patch = json!({
        "metadata": { "resourceVersion": resource_version },
        "spec": {
            "template": {
                "metadata": {
                    "annotations": {
                        "spacesly.dev/restart-token": desired_fingerprint.clone()
                    }
                }
            }
        }
    });
    let item =
        match client.patch_namespaced("apps", "v1", "deployments", &name, Some(&namespace), &patch)
        {
            Ok(item) => item,
            Err(error) => {
                let conflict = error.kind == OcpErrorKind::Conflict;
                let evidence = restart_evidence(
                    identity.clone(),
                    ResourceLookupStatus::DriftDetected,
                    observed_fingerprint.clone(),
                    Some(resource_version.clone()),
                    if conflict {
                        ResourceExecutionStatus::Conflict
                    } else {
                        ResourceExecutionStatus::Blocked
                    },
                    None,
                    None,
                    if conflict {
                        ResourceRetryResumeStatus::Conflict
                    } else {
                        ResourceRetryResumeStatus::AwaitingOperator
                    },
                )?;
                return Err(error.with_resource_mutation(evidence));
            }
        };
    let resulting_marker = item
        .pointer(RESTART_ANNOTATION_POINTER)
        .and_then(Value::as_str);
    if resulting_marker != Some(desired_fingerprint.as_str()) {
        let evidence = restart_evidence(
            identity,
            ResourceLookupStatus::DriftDetected,
            observed_fingerprint,
            Some(resource_version),
            ResourceExecutionStatus::Blocked,
            resulting_marker
                .map(|marker| state_fingerprint(&json!({ "restart_marker": marker })))
                .transpose()
                .map_err(|error| {
                    OcpError::internal(format!(
                        "Could not fingerprint Deployment restart result: {error}"
                    ))
                })?,
            item.pointer("/metadata/resourceVersion")
                .and_then(Value::as_str)
                .map(str::to_string),
            ResourceRetryResumeStatus::AwaitingOperator,
        )?;
        return Err(OcpError::api(
            "restart_result_incompatible",
            format!(
                "Deployment '{namespace}/{name}' restart response did not confirm the requested token."
            ),
        )
        .with_resource_mutation(evidence));
    }
    let evidence = restart_evidence(
        identity,
        ResourceLookupStatus::DriftDetected,
        observed_fingerprint,
        Some(resource_version),
        ResourceExecutionStatus::Executed,
        Some(desired_fingerprint),
        item.pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .map(str::to_string),
        ResourceRetryResumeStatus::ReconciledAfterDrift,
    )?;
    Ok(json!({
        "kind": "DeploymentRestart",
        "name": name,
        "namespace": namespace,
        "generation": item["metadata"]["generation"],
        "outcome": "applied",
        "resource_mutation": evidence
    }))
}

#[allow(clippy::too_many_arguments)]
fn restart_evidence(
    identity: ResourceOperationIdentity,
    lookup_status: ResourceLookupStatus,
    observed_fingerprint: Option<String>,
    observed_version: Option<String>,
    execution_status: ResourceExecutionStatus,
    resulting_fingerprint: Option<String>,
    resulting_version: Option<String>,
    retry_resume_status: ResourceRetryResumeStatus,
) -> OcpResult<ResourceMutationEvidence> {
    let evidence = ResourceMutationEvidence {
        identity,
        lookup: ResourceLookupResult {
            status: lookup_status,
            observed_fingerprint,
            observed_version,
        },
        execution: ResourceExecutionResult {
            status: execution_status,
            resulting_fingerprint,
            resulting_version,
        },
        retry_resume_status,
    };
    evidence.validate().map_err(|error| {
        OcpError::internal(format!("Deployment restart evidence was invalid: {error}"))
    })?;
    Ok(evidence)
}

fn scale_deployment(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let identity =
        resource_operation_identity(client, OcpTool::ScaleDeployment.as_str(), arguments)?
            .ok_or_else(|| OcpError::internal("Deployment scale identity was unavailable."))?;
    let namespace = identity
        .resource
        .namespace
        .clone()
        .ok_or_else(|| OcpError::internal("Deployment scale namespace was unavailable."))?;
    let name = identity.resource.name.clone();
    let replicas = required_u32(arguments, "replicas", 1000)?;
    let current = client
        .get_namespaced("apps", "v1", "deployments", &name, Some(&namespace))
        .map_err(|error| {
            error.with_resource_mutation(ResourceMutationEvidence {
                identity: identity.clone(),
                lookup: ResourceLookupResult {
                    status: ResourceLookupStatus::Unavailable,
                    observed_fingerprint: None,
                    observed_version: None,
                },
                execution: ResourceExecutionResult {
                    status: ResourceExecutionStatus::Blocked,
                    resulting_fingerprint: None,
                    resulting_version: None,
                },
                retry_resume_status: ResourceRetryResumeStatus::AwaitingOperator,
            })
        })?;
    let current_replicas = current
        .pointer("/spec/replicas")
        .and_then(Value::as_u64)
        .filter(|value| *value <= u64::from(u32::MAX))
        .map(|value| value as u32);
    let resource_version = current
        .pointer("/metadata/resourceVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let Some(current_replicas) = current_replicas else {
        let evidence = scale_evidence(
            identity,
            ResourceLookupStatus::Incompatible,
            None,
            resource_version,
            ResourceExecutionStatus::Conflict,
            None,
            None,
            ResourceRetryResumeStatus::Conflict,
        )?;
        return Err(OcpError::conflict(
            "incompatible_existing_resource",
            format!(
                "Deployment '{namespace}/{name}' has no compatible spec.replicas value; scaling was not attempted."
            ),
        )
        .with_resource_mutation(evidence));
    };
    let observed_fingerprint = replicas_fingerprint(current_replicas)?;
    if current_replicas == replicas {
        let evidence = scale_evidence(
            identity,
            ResourceLookupStatus::AlreadySatisfied,
            Some(observed_fingerprint),
            resource_version.clone(),
            ResourceExecutionStatus::Skipped,
            Some(replicas_fingerprint(replicas)?),
            resource_version,
            ResourceRetryResumeStatus::AlreadyComplete,
        )?;
        return Ok(json!({
            "kind": "DeploymentScale",
            "name": name,
            "namespace": namespace,
            "replicas": replicas,
            "outcome": "already_satisfied",
            "resource_mutation": evidence
        }));
    }
    let Some(resource_version) = resource_version else {
        let evidence = scale_evidence(
            identity,
            ResourceLookupStatus::Incompatible,
            Some(observed_fingerprint),
            None,
            ResourceExecutionStatus::Conflict,
            None,
            None,
            ResourceRetryResumeStatus::Conflict,
        )?;
        return Err(OcpError::conflict(
            "incompatible_existing_resource",
            format!(
                "Deployment '{namespace}/{name}' has no resourceVersion; scaling detected drift but was not attempted."
            ),
        )
        .with_resource_mutation(evidence));
    };
    let patch = json!({
        "metadata": { "resourceVersion": resource_version },
        "spec": { "replicas": replicas }
    });
    let item =
        match client.patch_namespaced("apps", "v1", "deployments", &name, Some(&namespace), &patch)
        {
            Ok(item) => item,
            Err(error) => {
                let conflict = error.kind == OcpErrorKind::Conflict;
                let evidence = scale_evidence(
                    identity.clone(),
                    ResourceLookupStatus::DriftDetected,
                    Some(observed_fingerprint.clone()),
                    Some(resource_version.clone()),
                    if conflict {
                        ResourceExecutionStatus::Conflict
                    } else {
                        ResourceExecutionStatus::Blocked
                    },
                    None,
                    None,
                    if conflict {
                        ResourceRetryResumeStatus::Conflict
                    } else {
                        ResourceRetryResumeStatus::AwaitingOperator
                    },
                )?;
                return Err(error.with_resource_mutation(evidence));
            }
        };
    let resulting_replicas = item.pointer("/spec/replicas").and_then(Value::as_u64);
    if resulting_replicas != Some(u64::from(replicas)) {
        let evidence = scale_evidence(
            identity,
            ResourceLookupStatus::DriftDetected,
            Some(observed_fingerprint),
            Some(resource_version),
            ResourceExecutionStatus::Blocked,
            resulting_replicas
                .filter(|value| *value <= u64::from(u32::MAX))
                .map(|value| replicas_fingerprint(value as u32))
                .transpose()?,
            item.pointer("/metadata/resourceVersion")
                .and_then(Value::as_str)
                .map(str::to_string),
            ResourceRetryResumeStatus::AwaitingOperator,
        )?;
        return Err(OcpError::api(
            "scale_result_incompatible",
            format!(
                "Deployment '{namespace}/{name}' scale response did not confirm the requested replica count."
            ),
        )
        .with_resource_mutation(evidence));
    }
    let resulting_version = item
        .pointer("/metadata/resourceVersion")
        .and_then(Value::as_str)
        .map(str::to_string);
    let evidence = scale_evidence(
        identity,
        ResourceLookupStatus::DriftDetected,
        Some(observed_fingerprint),
        Some(resource_version),
        ResourceExecutionStatus::Executed,
        Some(replicas_fingerprint(replicas)?),
        resulting_version,
        ResourceRetryResumeStatus::ReconciledAfterDrift,
    )?;
    Ok(json!({
        "kind": "DeploymentScale",
        "name": name,
        "namespace": namespace,
        "replicas": replicas,
        "generation": item["metadata"]["generation"],
        "outcome": "applied",
        "resource_mutation": evidence
    }))
}

#[allow(clippy::too_many_arguments)]
fn scale_evidence(
    identity: ResourceOperationIdentity,
    lookup_status: ResourceLookupStatus,
    observed_fingerprint: Option<String>,
    observed_version: Option<String>,
    execution_status: ResourceExecutionStatus,
    resulting_fingerprint: Option<String>,
    resulting_version: Option<String>,
    retry_resume_status: ResourceRetryResumeStatus,
) -> OcpResult<ResourceMutationEvidence> {
    let evidence = ResourceMutationEvidence {
        identity,
        lookup: ResourceLookupResult {
            status: lookup_status,
            observed_fingerprint,
            observed_version,
        },
        execution: ResourceExecutionResult {
            status: execution_status,
            resulting_fingerprint,
            resulting_version,
        },
        retry_resume_status,
    };
    evidence.validate().map_err(|error| {
        OcpError::internal(format!("Deployment scale evidence was invalid: {error}"))
    })?;
    Ok(evidence)
}

fn replicas_fingerprint(replicas: u32) -> OcpResult<String> {
    state_fingerprint(&json!({ "replicas": replicas })).map_err(|error| {
        OcpError::internal(format!(
            "Could not fingerprint Deployment replicas: {error}"
        ))
    })
}

fn generic_resource_list(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let resource = resolve_resource(client, arguments)?;
    let namespace = resource_namespace(&resource, arguments)?;
    let query = list_query(arguments, 100)?;
    let result = client.list_resource(&resource, namespace.as_deref(), &query)?;
    Ok(resource_response(
        "list",
        &resource,
        namespace.as_deref(),
        "result",
        result,
    ))
}

fn generic_resource_get(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let resource = resolve_resource(client, arguments)?;
    let namespace = resource_namespace(&resource, arguments)?;
    let name = required_resource_name(arguments, "name")?;
    let object = client.get_resource(&resource, namespace.as_deref(), &name)?;
    Ok(resource_response(
        "get",
        &resource,
        namespace.as_deref(),
        "object",
        object,
    ))
}

fn generic_resource_create(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let manifest = required_manifest(arguments)?;
    let (resource, namespace, _name) = validate_manifest_identity(client, manifest, false)?;
    let object = client.create_resource(&resource, namespace.as_deref(), manifest)?;
    Ok(resource_response(
        "create",
        &resource,
        namespace.as_deref(),
        "object",
        object,
    ))
}

fn generic_resource_update(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let manifest = required_manifest(arguments)?;
    let (resource, namespace, name) = validate_manifest_identity(client, manifest, true)?;
    let object = client.update_resource(&resource, namespace.as_deref(), &name, manifest)?;
    Ok(resource_response(
        "update",
        &resource,
        namespace.as_deref(),
        "object",
        object,
    ))
}

fn generic_resource_patch(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let resource = resolve_resource(client, arguments)?;
    let namespace = resource_namespace(&resource, arguments)?;
    let name = required_resource_name(arguments, "name")?;
    let patch = arguments.get("patch").ok_or_else(|| {
        OcpError::invalid_manifest("patch_required", "Structured patch content is required.")
    })?;
    let patch_type = match required_raw_string(arguments, "patch_type")?.as_str() {
        "merge" => {
            if !patch.is_object() {
                return Err(OcpError::invalid_manifest(
                    "merge_patch_invalid",
                    "JSON Merge Patch content must be an object.",
                ));
            }
            KubernetesPatchType::Merge
        }
        "json" => {
            validate_json_patch(patch)?;
            KubernetesPatchType::Json
        }
        "server_apply" => {
            validate_apply_patch_identity(patch, &resource, namespace.as_deref(), &name)?;
            KubernetesPatchType::ServerApply
        }
        value => {
            return Err(OcpError::invalid_manifest(
                "patch_type_invalid",
                format!("Unsupported patch_type '{value}'. Use merge, json, or server_apply."),
            ))
        }
    };
    let field_manager = optional_query_string(arguments, "field_manager")?;
    let force = arguments
        .get("force")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if patch_type != KubernetesPatchType::ServerApply && (field_manager.is_some() || force) {
        return Err(OcpError::invalid_manifest(
            "patch_options_invalid",
            "field_manager and force are only valid for server_apply.",
        ));
    }
    let object = client.patch_resource(
        &resource,
        namespace.as_deref(),
        &name,
        patch,
        patch_type,
        field_manager.as_deref(),
        force,
    )?;
    Ok(resource_response(
        "patch",
        &resource,
        namespace.as_deref(),
        "object",
        object,
    ))
}

fn generic_resource_delete(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let resource = resolve_resource(client, arguments)?;
    let namespace = resource_namespace(&resource, arguments)?;
    let name = required_resource_name(arguments, "name")?;
    let mut options = json!({ "apiVersion": "v1", "kind": "DeleteOptions" });
    if let Some(policy) = optional_query_string(arguments, "propagation_policy")? {
        if !matches!(policy.as_str(), "Foreground" | "Background" | "Orphan") {
            return Err(OcpError::config(
                "delete_options_invalid",
                "propagation_policy must be Foreground, Background, or Orphan.",
            ));
        }
        options["propagationPolicy"] = json!(policy);
    }
    if let Some(seconds) = arguments
        .get("grace_period_seconds")
        .and_then(Value::as_u64)
    {
        if seconds > 86_400 {
            return Err(OcpError::config(
                "delete_options_invalid",
                "grace_period_seconds must not exceed 86400.",
            ));
        }
        options["gracePeriodSeconds"] = json!(seconds);
    }
    if arguments
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        options["dryRun"] = json!(["All"]);
    }
    let result = client.delete_resource(&resource, namespace.as_deref(), &name, &options)?;
    Ok(resource_response(
        "delete",
        &resource,
        namespace.as_deref(),
        "result",
        result,
    ))
}

fn kubernetes_namespaces_list(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let resource = client.discover_resource("v1", "Namespace")?;
    let query = list_query(arguments, 100)?;
    let result = client.list_resource(&resource, None, &query)?;
    let items = result
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            json!({
                "name": item.pointer("/metadata/name"),
                "phase": item.pointer("/status/phase"),
                "labels": item.pointer("/metadata/labels").cloned().unwrap_or_else(|| json!({})),
                "annotations": item.pointer("/metadata/annotations").cloned().unwrap_or_else(|| json!({})),
                "creation_timestamp": item.pointer("/metadata/creationTimestamp")
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "api_version": "v1",
        "kind": "NamespaceList",
        "count": items.len(),
        "metadata": result.get("metadata").cloned().unwrap_or_else(|| json!({})),
        "items": items
    }))
}

fn kubernetes_events_list(client: &OcpClient, arguments: &Value) -> OcpResult<Value> {
    let (resource, modern) = match client.discover_resource("events.k8s.io/v1", "Event") {
        Ok(resource) => (resource, true),
        Err(error)
            if error.code == "api_version_not_found" || error.code == "api_resource_not_found" =>
        {
            (client.discover_resource("v1", "Event")?, false)
        }
        Err(error) => return Err(error),
    };
    let namespace = optional_resource_namespace(arguments)?;
    if let Some(namespace) = namespace.as_deref() {
        validate_identifier(namespace, "namespace")?;
    }
    let mut selectors = optional_query_string(arguments, "field_selector")?
        .map(|value| vec![value])
        .unwrap_or_default();
    let object_name = optional_query_string(arguments, "involved_object_name")?;
    let object_kind = optional_query_string(arguments, "involved_object_kind")?;
    let reason = optional_query_string(arguments, "reason")?;
    let event_type = optional_query_string(arguments, "type")?;
    for (field, value) in [
        (
            if modern {
                "regarding.name"
            } else {
                "involvedObject.name"
            },
            object_name,
        ),
        (
            if modern {
                "regarding.kind"
            } else {
                "involvedObject.kind"
            },
            object_kind,
        ),
        ("reason", reason),
        ("type", event_type),
    ] {
        if let Some(value) = value {
            selectors.push(format!("{field}={value}"));
        }
    }
    let mut query = list_query(arguments, 100)?;
    query.retain(|(key, _)| *key != "fieldSelector");
    if !selectors.is_empty() {
        query.push(("fieldSelector", selectors.join(",")));
    }
    let result = if namespace.is_some() {
        client.list_resource(&resource, namespace.as_deref(), &query)?
    } else {
        client.list_resource_all_namespaces(&resource, &query)?
    };
    let mut items = result
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| normalize_event(item, modern))
        .collect::<Vec<_>>();
    items.sort_by(|left, right| event_timestamp(right).cmp(event_timestamp(left)));
    Ok(json!({
        "api_version": resource.api_version,
        "kind": "EventList",
        "namespace": namespace,
        "count": items.len(),
        "metadata": result.get("metadata").cloned().unwrap_or_else(|| json!({})),
        "items": items
    }))
}

fn resolve_resource(client: &OcpClient, arguments: &Value) -> OcpResult<DiscoveredResource> {
    let api_version = required_raw_string(arguments, "api_version")?;
    let kind = optional_query_string(arguments, "kind")?;
    let resource = optional_query_string(arguments, "resource")?;
    let identity = match (kind.as_deref(), resource.as_deref()) {
        (Some(kind), None) => kind,
        (None, Some(resource)) => resource,
        (Some(_), Some(_)) => {
            return Err(OcpError::config(
                "resource_identity_ambiguous",
                "Supply either kind or resource, not both.",
            ))
        }
        (None, None) => {
            return Err(OcpError::config(
                "resource_identity_required",
                "Kubernetes resource kind or resource is required.",
            ))
        }
    };
    client.discover_resource(&api_version, identity)
}

fn resource_namespace(
    resource: &DiscoveredResource,
    arguments: &Value,
) -> OcpResult<Option<String>> {
    let namespace = optional_resource_namespace(arguments)?;
    if resource.namespaced && namespace.is_none() {
        return Err(OcpError::config(
            "namespace_required",
            format!(
                "Namespace is required for namespaced Kubernetes resource '{}'.",
                resource.qualified_name()
            ),
        ));
    }
    if !resource.namespaced && namespace.is_some() {
        return Err(OcpError::config(
            "namespace_not_allowed",
            format!(
                "Kubernetes resource '{}' is cluster-scoped; omit namespace.",
                resource.qualified_name()
            ),
        ));
    }
    if let Some(namespace) = namespace.as_deref() {
        validate_identifier(namespace, "namespace")?;
    }
    Ok(namespace)
}

fn validate_manifest_identity(
    client: &OcpClient,
    manifest: &Value,
    require_resource_version: bool,
) -> OcpResult<(DiscoveredResource, Option<String>, String)> {
    let api_version = manifest
        .get("apiVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            OcpError::invalid_manifest("api_version_required", "Manifest apiVersion is required.")
        })?;
    let kind = manifest
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| OcpError::invalid_manifest("kind_required", "Manifest kind is required."))?;
    let name = manifest
        .pointer("/metadata/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            OcpError::invalid_manifest(
                "name_required",
                "Manifest metadata.name is required; Spacesly does not infer it.",
            )
        })?
        .to_string();
    validate_identifier(&name, "metadata.name")?;
    let resource = client.discover_resource(api_version, kind)?;
    let namespace = manifest
        .pointer("/metadata/namespace")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if resource.namespaced && namespace.is_none() {
        return Err(OcpError::invalid_manifest(
            "namespace_required",
            format!(
                "Manifest metadata.namespace is required for namespaced resource '{}'.",
                resource.qualified_name()
            ),
        ));
    }
    if !resource.namespaced && namespace.is_some() {
        return Err(OcpError::invalid_manifest(
            "namespace_not_allowed",
            format!(
                "Manifest metadata.namespace must be omitted for cluster-scoped resource '{}'.",
                resource.qualified_name()
            ),
        ));
    }
    if let Some(namespace) = namespace.as_deref() {
        validate_identifier(namespace, "metadata.namespace")?;
    }
    if require_resource_version
        && manifest
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(OcpError::invalid_manifest(
            "resource_version_required",
            "Update requires metadata.resourceVersion from a current GET to prevent lost updates.",
        ));
    }
    Ok((resource, namespace, name))
}

fn required_manifest(arguments: &Value) -> OcpResult<&Value> {
    arguments
        .get("manifest")
        .filter(|manifest| manifest.is_object())
        .ok_or_else(|| {
            OcpError::invalid_manifest(
                "manifest_required",
                "A structured Kubernetes manifest object is required.",
            )
        })
}

fn validate_json_patch(patch: &Value) -> OcpResult<()> {
    let operations = patch.as_array().ok_or_else(|| {
        OcpError::invalid_manifest(
            "json_patch_invalid",
            "RFC 6902 JSON Patch content must be an array.",
        )
    })?;
    if operations.is_empty() {
        return Err(OcpError::invalid_manifest(
            "json_patch_empty",
            "RFC 6902 JSON Patch must contain at least one operation.",
        ));
    }
    for operation in operations {
        let op = operation.get("op").and_then(Value::as_str);
        let path = operation.get("path").and_then(Value::as_str);
        if !matches!(
            op,
            Some("add" | "remove" | "replace" | "move" | "copy" | "test")
        ) || path.is_none_or(|path| !path.starts_with('/'))
        {
            return Err(OcpError::invalid_manifest(
                "json_patch_invalid",
                "Each RFC 6902 operation requires a supported op and an absolute JSON Pointer path.",
            ));
        }
    }
    Ok(())
}

fn validate_apply_patch_identity(
    patch: &Value,
    resource: &DiscoveredResource,
    namespace: Option<&str>,
    name: &str,
) -> OcpResult<()> {
    if !patch.is_object()
        || patch.get("apiVersion").and_then(Value::as_str) != Some(resource.api_version.as_str())
        || patch.get("kind").and_then(Value::as_str) != Some(resource.kind.as_str())
        || patch.pointer("/metadata/name").and_then(Value::as_str) != Some(name)
        || patch.pointer("/metadata/namespace").and_then(Value::as_str) != namespace
    {
        return Err(OcpError::invalid_manifest(
            "apply_identity_mismatch",
            "Server-side apply content must include apiVersion, kind, metadata.name, and metadata.namespace matching the requested resource identity.",
        ));
    }
    Ok(())
}

fn resource_response(
    operation: &str,
    resource: &DiscoveredResource,
    namespace: Option<&str>,
    result_key: &str,
    result: Value,
) -> Value {
    let mut response = json!({
        "operation": operation,
        "resource": {
            "api_version": resource.api_version,
            "kind": resource.kind,
            "name": resource.name,
            "namespaced": resource.namespaced,
            "namespace": namespace
        }
    });
    response[result_key] = result;
    response
}

fn list_query(arguments: &Value, default_limit: u32) -> OcpResult<Vec<(&'static str, String)>> {
    let mut query = Vec::new();
    if let Some(selector) = optional_query_string(arguments, "label_selector")? {
        query.push(("labelSelector", selector));
    }
    if let Some(selector) = optional_query_string(arguments, "field_selector")? {
        query.push(("fieldSelector", selector));
    }
    let limit = optional_u32(arguments, "limit").unwrap_or(default_limit);
    if limit == 0 || limit as usize > MAX_LIST_ITEMS {
        return Err(OcpError::config(
            "limit_invalid",
            format!("Kubernetes list limit must be between 1 and {MAX_LIST_ITEMS}."),
        ));
    }
    query.push(("limit", limit.to_string()));
    if let Some(token) = optional_query_string(arguments, "continue")? {
        query.push(("continue", token));
    }
    Ok(query)
}

fn optional_resource_namespace(arguments: &Value) -> OcpResult<Option<String>> {
    optional_query_string(arguments, "namespace")
}

fn optional_query_string(arguments: &Value, key: &str) -> OcpResult<Option<String>> {
    let value = arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if value
        .as_deref()
        .is_some_and(|value| value.len() > 2048 || value.chars().any(char::is_control))
    {
        return Err(OcpError::config(
            "invalid_arguments",
            format!("Kubernetes tool argument '{key}' is invalid or too long."),
        ));
    }
    Ok(value)
}

fn required_raw_string(arguments: &Value, key: &str) -> OcpResult<String> {
    optional_query_string(arguments, key)?.ok_or_else(|| {
        OcpError::config(
            "invalid_arguments",
            format!("Kubernetes tool argument '{key}' is required."),
        )
    })
}

fn required_resource_name(arguments: &Value, key: &str) -> OcpResult<String> {
    let value = required_raw_string(arguments, key)?;
    validate_identifier(&value, key)?;
    Ok(value)
}

fn normalize_event(event: &Value, modern: bool) -> Value {
    let regarding = if modern {
        event.get("regarding")
    } else {
        event.get("involvedObject")
    };
    let source = if modern {
        event
            .get("reportingController")
            .or_else(|| event.pointer("/deprecatedSource/component"))
    } else {
        event
            .get("reportingComponent")
            .or_else(|| event.pointer("/source/component"))
    };
    let first_timestamp = if modern {
        event
            .get("eventTime")
            .filter(|value| !value.is_null())
            .or_else(|| event.get("deprecatedFirstTimestamp"))
            .or_else(|| event.pointer("/series/lastObservedTime"))
            .or_else(|| event.pointer("/metadata/creationTimestamp"))
    } else {
        event
            .get("firstTimestamp")
            .or_else(|| event.get("eventTime"))
            .or_else(|| event.pointer("/metadata/creationTimestamp"))
    };
    let last_timestamp = if modern {
        event
            .pointer("/series/lastObservedTime")
            .or_else(|| event.get("eventTime"))
            .filter(|value| !value.is_null())
            .or_else(|| event.get("deprecatedLastTimestamp"))
            .or_else(|| event.pointer("/metadata/creationTimestamp"))
    } else {
        event
            .get("lastTimestamp")
            .or_else(|| event.get("eventTime"))
            .or_else(|| event.pointer("/metadata/creationTimestamp"))
    };
    json!({
        "namespace": event.pointer("/metadata/namespace"),
        "type": event.get("type"),
        "reason": event.get("reason"),
        "message": if modern { event.get("note") } else { event.get("message") },
        "involved_object": {
            "api_version": regarding.and_then(|value| value.get("apiVersion")),
            "kind": regarding.and_then(|value| value.get("kind")),
            "namespace": regarding.and_then(|value| value.get("namespace")),
            "name": regarding.and_then(|value| value.get("name")),
            "uid": regarding.and_then(|value| value.get("uid"))
        },
        "reporting_controller": source,
        "count": if modern {
            event.pointer("/series/count").or_else(|| event.get("deprecatedCount"))
        } else {
            event.get("count").or_else(|| event.pointer("/series/count"))
        },
        "first_timestamp": first_timestamp,
        "last_timestamp": last_timestamp
    })
}

fn event_timestamp(event: &Value) -> &str {
    event
        .get("last_timestamp")
        .and_then(Value::as_str)
        .unwrap_or("")
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

fn required_restart_token(arguments: &Value) -> OcpResult<String> {
    let token = arguments
        .get("restart_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            OcpError::config(
                "restart_token_required",
                "Deployment restart requires a new lowercase UUIDv4 restart_token. Renew any approval created before tokenized restart support.",
            )
        })?;
    let bytes = token.as_bytes();
    let valid = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        })
        && bytes[14] == b'4'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b');
    if !valid {
        return Err(OcpError::config(
            "restart_token_invalid",
            "Deployment restart_token must be a lowercase UUIDv4.",
        ));
    }
    Ok(token.to_string())
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
        || value.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | ':'))
        })
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
    use crate::infrastructure::ocp::client::OcpTimeouts;
    use crate::infrastructure::ocp::config::{ClientCredentials, ResolvedCluster};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    const RESTART_TOKEN: &str = "11111111-1111-4111-8111-111111111111";
    const NEXT_RESTART_TOKEN: &str = "22222222-2222-4222-8222-222222222222";

    struct MockKubeServer {
        requests: Arc<Mutex<Vec<String>>>,
        handle: thread::JoinHandle<()>,
    }

    impl MockKubeServer {
        fn finish(self) -> Vec<String> {
            self.handle.join().expect("mock Kubernetes server joins");
            Arc::try_unwrap(self.requests)
                .expect("request log has one owner")
                .into_inner()
                .expect("request log lock")
        }
    }

    fn mock_client(responses: Vec<(u16, Value)>) -> (OcpClient, MockKubeServer) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener");
        let address = listener.local_addr().expect("mock address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_log = requests.clone();
        let handle = thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("mock request accepted");
                let request = read_http_request(&mut stream);
                request_log.lock().expect("request log lock").push(request);
                let body = serde_json::to_string(&body).expect("mock body encoded");
                let reason = match status {
                    200 => "OK",
                    201 => "Created",
                    403 => "Forbidden",
                    404 => "Not Found",
                    409 => "Conflict",
                    422 => "Unprocessable Entity",
                    _ => "Response",
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("mock response written");
            }
        });
        let cluster = ResolvedCluster {
            server: format!("http://{address}"),
            ca: None,
            insecure_skip_tls_verify: false,
            credentials: ClientCredentials::default(),
            default_namespace: Some("default".to_string()),
        };
        let client = OcpClient::build(&cluster, OcpTimeouts::default()).expect("test client");
        (client, MockKubeServer { requests, handle })
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("read timeout");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let count = stream.read(&mut buffer).expect("mock request read");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if expected_len.is_none() {
                if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected_len = Some(header_end + 4 + content_length);
                }
            }
            if expected_len.is_some_and(|expected| bytes.len() >= expected) {
                break;
            }
        }
        String::from_utf8(bytes).expect("request is UTF-8")
    }

    fn core_discovery() -> Value {
        json!({
            "groupVersion": "v1",
            "resources": [
                {"name":"pods","singularName":"pod","namespaced":true,"kind":"Pod","verbs":["get","list","create","update","patch","delete"],"shortNames":["po"]},
                {"name":"nodes","singularName":"node","namespaced":false,"kind":"Node","verbs":["get","list"]},
                {"name":"configmaps","singularName":"configmap","namespaced":true,"kind":"ConfigMap","verbs":["get","list","create","update","patch","delete"],"shortNames":["cm"]},
                {"name":"namespaces","singularName":"namespace","namespaced":false,"kind":"Namespace","verbs":["get","list"]},
                {"name":"events","singularName":"event","namespaced":true,"kind":"Event","verbs":["get","list"]}
            ]
        })
    }

    fn modern_event_discovery() -> Value {
        json!({
            "groupVersion": "events.k8s.io/v1",
            "resources": [
                {"name":"events","singularName":"event","namespaced":true,"kind":"Event","verbs":["get","list"]}
            ]
        })
    }

    fn status_error(code: u16, reason: &str, message: &str, kind: &str) -> Value {
        json!({
            "apiVersion": "v1",
            "kind": "Status",
            "status": "Failure",
            "message": message,
            "reason": reason,
            "details": { "kind": kind },
            "code": code
        })
    }

    fn deployment(name: &str, namespace: &str, replicas: Option<u32>, version: &str) -> Value {
        let mut value = json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {
                "name": name,
                "namespace": namespace,
                "resourceVersion": version,
                "generation": 1
            },
            "spec": {}
        });
        if let Some(replicas) = replicas {
            value["spec"]["replicas"] = json!(replicas);
        }
        value
    }

    fn restarted_deployment(name: &str, namespace: &str, token: &str, version: &str) -> Value {
        let mut value = deployment(name, namespace, Some(2), version);
        let marker = state_fingerprint(&json!({ "restart_token": token })).expect("marker");
        value["spec"]["template"]["metadata"]["annotations"] = json!({
            "spacesly.dev/restart-token": marker
        });
        value
    }

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
                "kubernetes_resources_create",
                "kubernetes_resources_update",
                "kubernetes_resources_patch",
                "kubernetes_resources_delete",
                "ocp_restart_deployment",
                "ocp_scale_deployment",
                "ocp_delete_managed_pod"
            ]
        );
    }

    #[test]
    fn restart_identity_is_stable_for_default_namespace_and_changes_with_token() {
        let omitted = restart_resource_operation_identity(
            "https://cluster.example:6443",
            "default",
            &json!({ "name": "api", "restart_token": RESTART_TOKEN }),
        )
        .expect("restart identity");
        let explicit = restart_resource_operation_identity(
            "https://cluster.example:6443",
            "other",
            &json!({
                "name": "api",
                "namespace": "default",
                "restart_token": RESTART_TOKEN
            }),
        )
        .expect("explicit identity");
        let next = restart_resource_operation_identity(
            "https://cluster.example:6443",
            "default",
            &json!({ "name": "api", "restart_token": NEXT_RESTART_TOKEN }),
        )
        .expect("next identity");

        assert_eq!(omitted, explicit);
        assert_ne!(omitted.key, next.key);
        assert_eq!(omitted.operation, "restart_deployment");
    }

    #[test]
    fn restart_identity_rejects_missing_or_non_uuid_tokens() {
        let missing = restart_resource_operation_identity(
            "https://cluster.example:6443",
            "default",
            &json!({ "name": "api" }),
        )
        .expect_err("missing token rejected");
        let invalid = restart_resource_operation_identity(
            "https://cluster.example:6443",
            "default",
            &json!({ "name": "api", "restart_token": "objective-1" }),
        )
        .expect_err("non-UUID token rejected");

        assert_eq!(missing.code, "restart_token_required");
        assert_eq!(invalid.code, "restart_token_invalid");
    }

    #[test]
    fn restart_deployment_looks_up_state_and_applies_token_once() {
        let restarted = restarted_deployment("api", "default", RESTART_TOKEN, "11");
        let (client, server) = mock_client(vec![
            (200, deployment("api", "default", Some(2), "10")),
            (200, restarted),
        ]);

        let result = execute_tool(
            &client,
            "ocp_restart_deployment",
            &json!({ "name": "api", "restart_token": RESTART_TOKEN }),
        )
        .expect("restart succeeds");
        let requests = server.finish();

        assert_eq!(result["outcome"], "applied");
        assert_eq!(
            result["resource_mutation"]["execution"]["status"],
            "executed"
        );
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /apis/apps/v1/namespaces/default/deployments/api"));
        assert!(requests[1].starts_with("PATCH /apis/apps/v1/namespaces/default/deployments/api"));
        assert!(requests[1].contains("\"resourceVersion\":\"10\""));
        assert!(requests[1].contains("\"spacesly.dev/restart-token\":\"sha256:"));
        assert!(!requests[1].contains(RESTART_TOKEN));
    }

    #[test]
    fn restart_deployment_identical_retry_is_already_complete_without_patch() {
        let (client, server) = mock_client(vec![(
            200,
            restarted_deployment("api", "default", RESTART_TOKEN, "11"),
        )]);

        let result = execute_tool(
            &client,
            "ocp_restart_deployment",
            &json!({ "name": "api", "restart_token": RESTART_TOKEN }),
        )
        .expect("restart retry reconciles");
        let requests = server.finish();

        assert_eq!(result["outcome"], "already_satisfied");
        assert_eq!(
            result["resource_mutation"]["execution"]["status"],
            "skipped"
        );
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET "));
    }

    #[test]
    fn restart_deployment_preserves_resource_version_conflict_evidence() {
        let (client, server) = mock_client(vec![
            (200, deployment("api", "default", Some(2), "10")),
            (
                409,
                status_error(409, "Conflict", "object was modified", "deployments"),
            ),
        ]);

        let error = execute_tool(
            &client,
            "ocp_restart_deployment",
            &json!({ "name": "api", "restart_token": RESTART_TOKEN }),
        )
        .expect_err("restart conflict retained");
        let requests = server.finish();
        let evidence = error.resource_mutation.expect("restart evidence");

        assert_eq!(error.kind, OcpErrorKind::Conflict);
        assert_eq!(evidence.execution.status, ResourceExecutionStatus::Conflict);
        assert_eq!(evidence.lookup.observed_version.as_deref(), Some("10"));
        assert!(requests[1].contains("\"resourceVersion\":\"10\""));
    }

    #[test]
    fn scale_deployment_looks_up_state_and_applies_drift_once() {
        let mut scaled = deployment("api", "default", Some(3), "11");
        scaled["metadata"]["generation"] = json!(2);
        let (client, server) = mock_client(vec![
            (200, deployment("api", "default", Some(2), "10")),
            (200, scaled),
        ]);

        let result = execute_tool(
            &client,
            "ocp_scale_deployment",
            &json!({"name":"api","replicas":3}),
        )
        .unwrap();

        assert_eq!(result["outcome"], "applied");
        assert_eq!(
            result["resource_mutation"]["lookup"]["status"],
            "drift_detected"
        );
        assert_eq!(
            result["resource_mutation"]["execution"]["status"],
            "executed"
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(requests[0]
            .starts_with("GET /apis/apps/v1/namespaces/default/deployments/api HTTP/1.1"));
        assert!(requests[1]
            .starts_with("PATCH /apis/apps/v1/namespaces/default/deployments/api HTTP/1.1"));
        assert!(requests[1].contains("\"resourceVersion\":\"10\""));
        assert!(requests[1].contains("\"replicas\":3"));
    }

    #[test]
    fn scale_deployment_identical_retry_is_already_complete_without_patch() {
        let (client, server) =
            mock_client(vec![(200, deployment("api", "default", Some(3), "11"))]);
        let implicit = resource_operation_identity(
            &client,
            "ocp_scale_deployment",
            &json!({"name":"api","replicas":3}),
        )
        .unwrap()
        .unwrap();
        let explicit = resource_operation_identity(
            &client,
            "ocp_scale_deployment",
            &json!({"namespace":"default","name":"api","replicas":3}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(implicit.key, explicit.key);

        let result = execute_tool(
            &client,
            "ocp_scale_deployment",
            &json!({"namespace":"default","name":"api","replicas":3}),
        )
        .unwrap();
        assert_eq!(result["outcome"], "already_satisfied");
        assert_eq!(
            result["resource_mutation"]["retry_resume_status"],
            "already_complete"
        );
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn scale_deployment_resume_reconciles_partial_completion_without_second_patch() {
        let (client, server) =
            mock_client(vec![(200, deployment("api", "payments", Some(4), "22"))]);

        let result = execute_tool(
            &client,
            "ocp_scale_deployment",
            &json!({"namespace":"payments","name":"api","replicas":4}),
        )
        .unwrap();

        assert_eq!(result["outcome"], "already_satisfied");
        assert_eq!(
            result["resource_mutation"]["execution"]["status"],
            "skipped"
        );
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .starts_with("GET /apis/apps/v1/namespaces/payments/deployments/api HTTP/1.1"));
    }

    #[test]
    fn scale_deployment_rejects_incompatible_existing_resource() {
        let (client, server) = mock_client(vec![(200, deployment("api", "default", None, "10"))]);

        let error = execute_tool(
            &client,
            "ocp_scale_deployment",
            &json!({"name":"api","replicas":3}),
        )
        .unwrap_err();

        assert_eq!(error.kind, OcpErrorKind::Conflict);
        assert_eq!(error.code, "incompatible_existing_resource");
        assert_eq!(
            error.resource_mutation.unwrap().lookup.status,
            ResourceLookupStatus::Incompatible
        );
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn scale_deployment_preserves_resource_version_conflict_evidence() {
        let (client, server) = mock_client(vec![
            (200, deployment("api", "default", Some(2), "10")),
            (
                409,
                status_error(
                    409,
                    "Conflict",
                    "the object has been modified",
                    "deployments",
                ),
            ),
        ]);

        let error = execute_tool(
            &client,
            "ocp_scale_deployment",
            &json!({"name":"api","replicas":3}),
        )
        .unwrap_err();

        assert_eq!(error.kind, OcpErrorKind::Conflict);
        assert_eq!(error.code, "resource_version_conflict");
        let evidence = error.resource_mutation.unwrap();
        assert_eq!(evidence.lookup.status, ResourceLookupStatus::DriftDetected);
        assert_eq!(evidence.execution.status, ResourceExecutionStatus::Conflict);
        assert_eq!(
            evidence.retry_resume_status,
            ResourceRetryResumeStatus::Conflict
        );
        assert_eq!(server.finish().len(), 2);
    }

    #[test]
    fn scale_deployment_unauthorized_lookup_blocks_without_mutation() {
        let (client, server) = mock_client(vec![(
            403,
            status_error(403, "Forbidden", "deployments is forbidden", "deployments"),
        )]);

        let error = execute_tool(
            &client,
            "ocp_scale_deployment",
            &json!({"name":"api","replicas":3}),
        )
        .unwrap_err();

        assert_eq!(error.kind, OcpErrorKind::Forbidden);
        let evidence = error.resource_mutation.unwrap();
        assert_eq!(evidence.lookup.status, ResourceLookupStatus::Unavailable);
        assert_eq!(evidence.execution.status, ResourceExecutionStatus::Blocked);
        assert_eq!(server.finish().len(), 1);
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
        assert!(validate_identifier("system:node", "name").is_ok());
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

    #[test]
    fn generic_crud_preserves_scope_payloads_and_discovery_cache() {
        let (client, server) = mock_client(vec![
            (200, core_discovery()),
            (
                200,
                json!({"kind":"PodList","metadata":{"continue":"next"},"items":[{"apiVersion":"v1","kind":"Pod","metadata":{"name":"web","namespace":"team-a"}}]}),
            ),
            (
                200,
                json!({"kind":"NodeList","items":[{"apiVersion":"v1","kind":"Node","metadata":{"name":"minikube"}}]}),
            ),
            (
                200,
                json!({"apiVersion":"v1","kind":"Pod","metadata":{"name":"web","namespace":"team-a"},"spec":{},"status":{"phase":"Running"}}),
            ),
            (
                201,
                json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"team-a","resourceVersion":"1"},"data":{"A":"1"}}),
            ),
            (
                200,
                json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"team-a","resourceVersion":"2"},"data":{"A":"2"}}),
            ),
            (
                200,
                json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"team-a","resourceVersion":"3"},"data":{"A":"3"}}),
            ),
            (200, json!({"kind":"Status","status":"Success"})),
        ]);

        let listed = execute_tool(
            &client,
            "kubernetes_resources_list",
            &json!({"api_version":"v1","kind":"Pod","namespace":"team-a","label_selector":"app=web","limit":20}),
        )
        .unwrap();
        assert_eq!(listed["result"]["items"][0]["metadata"]["name"], "web");
        assert_eq!(listed["result"]["metadata"]["continue"], "next");

        let nodes = execute_tool(
            &client,
            "kubernetes_resources_list",
            &json!({"api_version":"v1","resource":"nodes"}),
        )
        .unwrap();
        assert_eq!(nodes["resource"]["namespaced"], false);

        let pod = execute_tool(
            &client,
            "kubernetes_resources_get",
            &json!({"api_version":"v1","resource":"po","namespace":"team-a","name":"web"}),
        )
        .unwrap();
        assert_eq!(pod["object"]["status"]["phase"], "Running");

        let created = execute_tool(
            &client,
            "kubernetes_resources_create",
            &json!({"manifest":{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"team-a"},"data":{"A":"1"}}}),
        )
        .unwrap();
        assert_eq!(created["object"]["metadata"]["resourceVersion"], "1");

        let updated = execute_tool(
            &client,
            "kubernetes_resources_update",
            &json!({"manifest":{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"team-a","resourceVersion":"1"},"data":{"A":"2"}}}),
        )
        .unwrap();
        assert_eq!(updated["object"]["metadata"]["resourceVersion"], "2");

        let patched = execute_tool(
            &client,
            "kubernetes_resources_patch",
            &json!({"api_version":"v1","kind":"ConfigMap","namespace":"team-a","name":"settings","patch_type":"merge","patch":{"data":{"A":"3"}}}),
        )
        .unwrap();
        assert_eq!(patched["object"]["data"]["A"], "3");

        let deleted = execute_tool(
            &client,
            "kubernetes_resources_delete",
            &json!({"api_version":"v1","kind":"ConfigMap","namespace":"team-a","name":"settings","propagation_policy":"Background"}),
        )
        .unwrap();
        assert_eq!(deleted["result"]["status"], "Success");

        let requests = server.finish();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("GET /api/v1 HTTP/1.1"))
                .count(),
            1
        );
        assert!(requests[1].starts_with("GET /api/v1/namespaces/team-a/pods?"));
        assert!(requests[1].contains("labelSelector=app%3Dweb"));
        assert!(requests[2].starts_with("GET /api/v1/nodes?"));
        assert!(requests[3].starts_with("GET /api/v1/namespaces/team-a/pods/web HTTP/1.1"));
        assert!(requests[4].starts_with("POST /api/v1/namespaces/team-a/configmaps HTTP/1.1"));
        assert!(
            requests[5].starts_with("PUT /api/v1/namespaces/team-a/configmaps/settings HTTP/1.1")
        );
        assert!(
            requests[6].starts_with("PATCH /api/v1/namespaces/team-a/configmaps/settings HTTP/1.1")
        );
        assert!(requests[6]
            .to_ascii_lowercase()
            .contains("content-type: application/merge-patch+json"));
        assert!(requests[7]
            .starts_with("DELETE /api/v1/namespaces/team-a/configmaps/settings HTTP/1.1"));
        assert!(requests[7].contains("\"propagationPolicy\":\"Background\""));
    }

    #[test]
    fn generic_resources_validate_scope_kind_and_update_concurrency() {
        let (client, server) = mock_client(vec![(200, core_discovery())]);
        let missing_namespace = execute_tool(
            &client,
            "kubernetes_resources_list",
            &json!({"api_version":"v1","kind":"Pod"}),
        )
        .unwrap_err();
        assert_eq!(missing_namespace.code, "namespace_required");

        let cluster_namespace = execute_tool(
            &client,
            "kubernetes_resources_list",
            &json!({"api_version":"v1","kind":"Node","namespace":"default"}),
        )
        .unwrap_err();
        assert_eq!(cluster_namespace.code, "namespace_not_allowed");

        let unknown = execute_tool(
            &client,
            "kubernetes_resources_list",
            &json!({"api_version":"v1","kind":"Mystery"}),
        )
        .unwrap_err();
        assert_eq!(unknown.code, "api_resource_not_found");

        let stale_update = execute_tool(
            &client,
            "kubernetes_resources_update",
            &json!({"manifest":{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"default"}}}),
        )
        .unwrap_err();
        assert_eq!(stale_update.code, "resource_version_required");
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn generic_resources_preserve_forbidden_and_conflict_diagnostics() {
        let (client, server) = mock_client(vec![
            (200, core_discovery()),
            (
                403,
                status_error(403, "Forbidden", "pods is forbidden", "pods"),
            ),
            (
                409,
                status_error(
                    409,
                    "Conflict",
                    "the object has been modified",
                    "configmaps",
                ),
            ),
        ]);
        let forbidden = execute_tool(
            &client,
            "kubernetes_resources_list",
            &json!({"api_version":"v1","kind":"Pod","namespace":"locked"}),
        )
        .unwrap_err();
        assert_eq!(
            forbidden.kind,
            crate::infrastructure::ocp::errors::OcpErrorKind::Forbidden
        );
        assert!(forbidden.message.contains("verb 'list'"));
        assert!(forbidden.message.contains("'pods'"));
        assert!(forbidden.message.contains("namespace 'locked'"));

        let conflict = execute_tool(
            &client,
            "kubernetes_resources_update",
            &json!({"manifest":{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"locked","resourceVersion":"old"}}}),
        )
        .unwrap_err();
        assert_eq!(
            conflict.kind,
            crate::infrastructure::ocp::errors::OcpErrorKind::Conflict
        );
        assert_eq!(conflict.code, "resource_version_conflict");
        assert!(conflict.message.contains("current resourceVersion"));
        assert_eq!(server.finish().len(), 3);
    }

    #[test]
    fn namespace_listing_returns_operational_metadata() {
        let (client, server) = mock_client(vec![
            (200, core_discovery()),
            (
                200,
                json!({"kind":"NamespaceList","metadata":{"continue":"more"},"items":[{"metadata":{"name":"team-a","labels":{"env":"test"},"annotations":{"owner":"platform"},"creationTimestamp":"2026-08-01T00:00:00Z"},"status":{"phase":"Active"}}]}),
            ),
        ]);
        let result = execute_tool(
            &client,
            "kubernetes_namespaces_list",
            &json!({"label_selector":"env=test","limit":10}),
        )
        .unwrap();
        assert_eq!(result["items"][0]["name"], "team-a");
        assert_eq!(result["items"][0]["phase"], "Active");
        assert_eq!(result["items"][0]["labels"]["env"], "test");
        assert_eq!(result["metadata"]["continue"], "more");
        let requests = server.finish();
        assert!(requests[1].starts_with("GET /api/v1/namespaces?"));
    }

    #[test]
    fn events_use_modern_api_namespace_and_all_namespace_filters() {
        let event = json!({
            "apiVersion":"events.k8s.io/v1","kind":"Event",
            "metadata":{"namespace":"team-a","creationTimestamp":"2026-08-08T01:00:00Z"},
            "type":"Warning","reason":"BackOff","note":"container is restarting",
            "regarding":{"apiVersion":"v1","kind":"Pod","namespace":"team-a","name":"web-1","uid":"u1"},
            "reportingController":"kubelet","eventTime":null,
            "deprecatedFirstTimestamp":"2026-08-08T02:00:00Z",
            "deprecatedLastTimestamp":"2026-08-08T03:00:00Z",
            "deprecatedCount":4
        });
        let (client, server) = mock_client(vec![
            (200, modern_event_discovery()),
            (200, json!({"kind":"EventList","items":[event.clone()]})),
            (200, json!({"kind":"EventList","items":[event]})),
        ]);
        let namespaced = execute_tool(
            &client,
            "kubernetes_events_list",
            &json!({"namespace":"team-a","involved_object_name":"web-1","involved_object_kind":"Pod","reason":"BackOff","type":"Warning","limit":25}),
        )
        .unwrap();
        assert_eq!(namespaced["items"][0]["message"], "container is restarting");
        assert_eq!(namespaced["items"][0]["reporting_controller"], "kubelet");
        assert_eq!(namespaced["items"][0]["count"], 4);
        assert_eq!(
            namespaced["items"][0]["first_timestamp"],
            "2026-08-08T02:00:00Z"
        );
        assert_eq!(
            namespaced["items"][0]["last_timestamp"],
            "2026-08-08T03:00:00Z"
        );

        let all = execute_tool(&client, "kubernetes_events_list", &json!({"limit":25})).unwrap();
        assert_eq!(all["count"], 1);
        let requests = server.finish();
        assert!(requests[1].starts_with("GET /apis/events.k8s.io/v1/namespaces/team-a/events?"));
        assert!(requests[1].contains("fieldSelector="));
        assert!(requests[1].contains("regarding.name%3Dweb-1"));
        assert!(requests[2].starts_with("GET /apis/events.k8s.io/v1/events?"));
    }

    #[test]
    fn events_fall_back_to_core_v1_and_custom_resources_are_generic() {
        let widget_discovery = json!({
            "groupVersion":"example.io/v1",
            "resources":[{"name":"widgets","singularName":"widget","namespaced":true,"kind":"Widget","verbs":["get","list","create","update","patch","delete"],"shortNames":["wd"]}]
        });
        let (client, server) = mock_client(vec![
            (
                404,
                status_error(
                    404,
                    "NotFound",
                    "the server could not find the requested resource",
                    "events",
                ),
            ),
            (200, core_discovery()),
            (
                200,
                json!({"kind":"EventList","items":[{"metadata":{"namespace":"team-a","creationTimestamp":"2026-08-08T01:00:00Z"},"type":"Normal","reason":"Scheduled","message":"assigned","involvedObject":{"kind":"Pod","name":"web"},"source":{"component":"scheduler"},"count":1,"lastTimestamp":"2026-08-08T02:00:00Z"}]}),
            ),
            (200, widget_discovery),
            (
                200,
                json!({"apiVersion":"example.io/v1","kind":"WidgetList","items":[{"apiVersion":"example.io/v1","kind":"Widget","metadata":{"name":"sample","namespace":"team-a"},"spec":{"size":2}}]}),
            ),
        ]);
        let events = execute_tool(
            &client,
            "kubernetes_events_list",
            &json!({"namespace":"team-a","involved_object_name":"web"}),
        )
        .unwrap();
        assert_eq!(events["api_version"], "v1");
        assert_eq!(events["items"][0]["reporting_controller"], "scheduler");

        let widgets = execute_tool(
            &client,
            "kubernetes_resources_list",
            &json!({"api_version":"example.io/v1","resource":"wd","namespace":"team-a"}),
        )
        .unwrap();
        assert_eq!(widgets["result"]["items"][0]["kind"], "Widget");
        let requests = server.finish();
        assert!(requests[0].starts_with("GET /apis/events.k8s.io/v1 HTTP/1.1"));
        assert!(requests[2].contains("involvedObject.name%3Dweb"));
        assert!(requests[3].starts_with("GET /apis/example.io/v1 HTTP/1.1"));
        assert!(requests[4].starts_with("GET /apis/example.io/v1/namespaces/team-a/widgets?"));
    }

    #[test]
    fn patch_validation_distinguishes_json_merge_and_server_apply() {
        assert!(
            validate_json_patch(&json!([{"op":"replace","path":"/data/A","value":"2"}])).is_ok()
        );
        assert!(validate_json_patch(&json!({"data":{"A":"2"}})).is_err());
        assert!(validate_json_patch(&json!([{"op":"replace","path":"relative"}])).is_err());
    }

    #[test]
    fn patch_types_use_distinct_kubernetes_protocol_semantics() {
        let (client, server) = mock_client(vec![
            (200, core_discovery()),
            (
                200,
                json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"default"}}),
            ),
            (
                200,
                json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"default"}}),
            ),
        ]);
        execute_tool(
            &client,
            "kubernetes_resources_patch",
            &json!({
                "api_version":"v1","kind":"ConfigMap","namespace":"default","name":"settings",
                "patch_type":"json","patch":[{"op":"replace","path":"/data/A","value":"2"}]
            }),
        )
        .unwrap();
        execute_tool(
            &client,
            "kubernetes_resources_patch",
            &json!({
                "api_version":"v1","kind":"ConfigMap","namespace":"default","name":"settings",
                "patch_type":"server_apply","field_manager":"spacesly","force":true,
                "patch":{"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"settings","namespace":"default"},"data":{"A":"3"}}
            }),
        )
        .unwrap();
        let requests = server.finish();
        assert!(requests[1]
            .to_ascii_lowercase()
            .contains("content-type: application/json-patch+json"));
        assert!(requests[1].contains("\"op\":\"replace\""));
        assert!(requests[2].starts_with(
            "PATCH /api/v1/namespaces/default/configmaps/settings?fieldManager=spacesly&force=true"
        ));
        assert!(requests[2]
            .to_ascii_lowercase()
            .contains("content-type: application/apply-patch+yaml"));
    }
}
