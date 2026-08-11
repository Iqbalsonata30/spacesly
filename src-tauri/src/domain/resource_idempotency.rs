//! Provider-neutral identities and evidence for deterministic resource mutations.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const RESOURCE_OPERATION_IDENTITY_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceIdentity {
    pub api_version: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceOperationIdentity {
    pub schema_version: u32,
    pub connector: String,
    pub operation: String,
    pub resource: ResourceIdentity,
    pub environment_fingerprint: String,
    pub mutation_fingerprint: String,
    pub key: String,
}

impl ResourceOperationIdentity {
    pub fn new(
        connector: &str,
        operation: &str,
        resource: ResourceIdentity,
        environment: &str,
        desired_state: &Value,
    ) -> Result<Self, String> {
        let connector = normalize_component(connector, "connector")?;
        let operation = normalize_component(operation, "operation")?;
        validate_resource(&resource)?;
        if environment.trim().is_empty() {
            return Err("Resource operation environment is required.".to_string());
        }
        let environment_fingerprint = digest_bytes(environment.trim().as_bytes());
        let mutation_fingerprint = digest_json(desired_state)?;
        let mut identity = Self {
            schema_version: RESOURCE_OPERATION_IDENTITY_SCHEMA_VERSION,
            connector,
            operation,
            resource,
            environment_fingerprint,
            mutation_fingerprint,
            key: String::new(),
        };
        identity.key =
            digest_json(&serde_json::to_value(&identity).map_err(|error| {
                format!("Failed to encode resource operation identity: {error}")
            })?)?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != RESOURCE_OPERATION_IDENTITY_SCHEMA_VERSION {
            return Err("Resource operation identity schema is unsupported.".to_string());
        }
        normalize_component(&self.connector, "connector")?;
        normalize_component(&self.operation, "operation")?;
        validate_resource(&self.resource)?;
        for (field, value) in [
            ("environment fingerprint", &self.environment_fingerprint),
            ("mutation fingerprint", &self.mutation_fingerprint),
            ("operation key", &self.key),
        ] {
            if !valid_digest(value) {
                return Err(format!("Resource operation {field} is invalid."));
            }
        }
        let mut expected = self.clone();
        expected.key.clear();
        let expected_key =
            digest_json(&serde_json::to_value(&expected).map_err(|error| {
                format!("Failed to encode resource operation identity: {error}")
            })?)?;
        if self.key != expected_key {
            return Err("Resource operation key does not match its identity.".to_string());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceLookupStatus {
    AlreadySatisfied,
    DriftDetected,
    Incompatible,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceLookupResult {
    pub status: ResourceLookupStatus,
    pub observed_fingerprint: Option<String>,
    pub observed_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceExecutionStatus {
    Executed,
    Skipped,
    Blocked,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceExecutionResult {
    pub status: ResourceExecutionStatus,
    pub resulting_fingerprint: Option<String>,
    pub resulting_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRetryResumeStatus {
    FirstExecution,
    AlreadyComplete,
    ReconciledAfterDrift,
    AwaitingApproval,
    AwaitingOperator,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceMutationEvidence {
    pub identity: ResourceOperationIdentity,
    pub lookup: ResourceLookupResult,
    pub execution: ResourceExecutionResult,
    pub retry_resume_status: ResourceRetryResumeStatus,
}

impl ResourceMutationEvidence {
    pub fn validate(&self) -> Result<(), String> {
        self.identity.validate()?;
        for value in [
            self.lookup.observed_fingerprint.as_ref(),
            self.execution.resulting_fingerprint.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !valid_digest(value) {
                return Err("Resource mutation evidence fingerprint is invalid.".to_string());
            }
        }
        for value in [
            self.lookup.observed_version.as_ref(),
            self.execution.resulting_version.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                return Err("Resource mutation evidence version is invalid.".to_string());
            }
        }
        Ok(())
    }
}

pub fn state_fingerprint(state: &Value) -> Result<String, String> {
    digest_json(state)
}

fn normalize_component(value: &str, field: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty()
        || normalized.len() > 128
        || normalized
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')))
    {
        return Err(format!("Resource operation {field} is invalid."));
    }
    Ok(normalized)
}

fn validate_resource(resource: &ResourceIdentity) -> Result<(), String> {
    for (field, value) in [
        ("api version", resource.api_version.as_str()),
        ("kind", resource.kind.as_str()),
        ("name", resource.name.as_str()),
    ] {
        if value.is_empty() || value.len() > 253 || value.chars().any(char::is_control) {
            return Err(format!("Resource identity {field} is invalid."));
        }
    }
    if resource.namespace.as_ref().is_some_and(|namespace| {
        namespace.is_empty() || namespace.len() > 253 || namespace.chars().any(char::is_control)
    }) {
        return Err("Resource identity namespace is invalid.".to_string());
    }
    Ok(())
}

fn digest_json(value: &Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(&canonical_json(value))
        .map_err(|error| format!("Failed to encode canonical resource state: {error}"))?;
    Ok(digest_bytes(&bytes))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonical_json(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        value => value.clone(),
    }
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn identity(desired: &Value) -> ResourceOperationIdentity {
        ResourceOperationIdentity::new(
            "openshift_kubernetes",
            "scale_deployment",
            ResourceIdentity {
                api_version: "apps/v1".to_string(),
                kind: "Deployment".to_string(),
                namespace: Some("payments".to_string()),
                name: "api".to_string(),
            },
            "https://cluster.example:6443",
            desired,
        )
        .unwrap()
    }

    #[test]
    fn identity_is_stable_for_canonical_desired_state() {
        let first = identity(&json!({"replicas": 3, "strategy": "safe"}));
        let second = identity(&json!({"strategy": "safe", "replicas": 3}));
        assert_eq!(first, second);
        first.validate().unwrap();
    }

    #[test]
    fn identity_changes_with_resource_or_desired_state() {
        let first = identity(&json!({"replicas": 3}));
        let second = identity(&json!({"replicas": 4}));
        assert_ne!(first.key, second.key);
        assert_ne!(first.mutation_fingerprint, second.mutation_fingerprint);
    }

    #[test]
    fn identity_never_serializes_environment_or_desired_sensitive_values() {
        let secret = "sensitive-cluster-token";
        let identity = ResourceOperationIdentity::new(
            "connector",
            "operation",
            ResourceIdentity {
                api_version: "v1".to_string(),
                kind: "Secret".to_string(),
                namespace: Some("default".to_string()),
                name: "settings".to_string(),
            },
            secret,
            &json!({"token": secret}),
        )
        .unwrap();
        let encoded = serde_json::to_string(&identity).unwrap();
        assert!(!encoded.contains(secret));
        identity.validate().unwrap();
    }
}
