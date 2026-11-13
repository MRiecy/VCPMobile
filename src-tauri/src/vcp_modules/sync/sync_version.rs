use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

pub(super) const WIRE_PROTOCOL_VERSION: &str = "1.6";
const MAX_VERSION_TOKEN_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VersionComponent {
    MobileApp,
    DesktopPlugin,
    Wire,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct VersionClaim {
    component: VersionComponent,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VersionCheckFrame {
    #[serde(rename = "type")]
    frame_type: &'static str,
    versions: Vec<VersionClaim>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VersionAckFrame {
    #[serde(rename = "type")]
    frame_type: String,
    versions: Vec<VersionClaim>,
    backend_mode: DesktopBackendMode,
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum DesktopBackendMode {
    Legacy,
    Cds,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct AcceptedDesktopVersions {
    pub(super) package_version: String,
    pub(super) wire_version: String,
    pub(super) backend_mode: DesktopBackendMode,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum VersionContractError {
    Invalid(String),
    Mismatch {
        expected: &'static str,
        received: String,
        package_version: String,
    },
}

impl fmt::Display for VersionContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Mismatch {
                expected, received, ..
            } => write!(
                formatter,
                "wire protocol mismatch: expected {expected}, received {received}"
            ),
        }
    }
}

pub(super) fn build_version_check(mobile_app_version: &str) -> Result<VersionCheckFrame, String> {
    validate_version_token(mobile_app_version, "mobile_app")?;
    validate_version_token(WIRE_PROTOCOL_VERSION, "wire")?;
    Ok(VersionCheckFrame {
        frame_type: "VERSION_CHECK",
        versions: vec![
            VersionClaim {
                component: VersionComponent::MobileApp,
                version: mobile_app_version.to_string(),
            },
            VersionClaim {
                component: VersionComponent::Wire,
                version: WIRE_PROTOCOL_VERSION.to_string(),
            },
        ],
    })
}

pub(super) fn parse_version_ack(
    text: &str,
) -> Result<AcceptedDesktopVersions, VersionContractError> {
    let ack = serde_json::from_str::<VersionAckFrame>(text)
        .map_err(|error| VersionContractError::Invalid(format!("Invalid VERSION_ACK: {error}")))?;
    if ack.frame_type != "VERSION_ACK" {
        return Err(VersionContractError::Invalid(
            "expected VERSION_ACK".to_string(),
        ));
    }

    let mut versions = validate_claims(
        ack.versions,
        &[VersionComponent::DesktopPlugin, VersionComponent::Wire],
        "VERSION_ACK",
    )
    .map_err(VersionContractError::Invalid)?;
    let package_version = versions
        .remove(&VersionComponent::DesktopPlugin)
        .ok_or_else(|| {
            VersionContractError::Invalid(
                "VERSION_ACK is missing desktop_plugin version".to_string(),
            )
        })?;
    let wire_version = versions.remove(&VersionComponent::Wire).ok_or_else(|| {
        VersionContractError::Invalid("VERSION_ACK is missing wire version".to_string())
    })?;

    if wire_version != WIRE_PROTOCOL_VERSION {
        return Err(VersionContractError::Mismatch {
            expected: WIRE_PROTOCOL_VERSION,
            received: wire_version,
            package_version,
        });
    }

    Ok(AcceptedDesktopVersions {
        package_version,
        wire_version,
        backend_mode: ack.backend_mode,
    })
}

fn validate_claims(
    claims: Vec<VersionClaim>,
    expected: &[VersionComponent],
    label: &str,
) -> Result<HashMap<VersionComponent, String>, String> {
    if claims.len() != expected.len() {
        return Err(format!(
            "{label}.versions must contain exactly {} entries",
            expected.len()
        ));
    }

    let mut versions = HashMap::with_capacity(expected.len());
    for claim in claims {
        if !expected.contains(&claim.component) {
            return Err(format!(
                "{label}.versions contains unexpected component {:?}",
                claim.component
            ));
        }
        validate_version_token(&claim.version, "version")?;
        if versions.insert(claim.component, claim.version).is_some() {
            return Err(format!(
                "{label}.versions contains duplicate component {:?}",
                claim.component
            ));
        }
    }

    if expected
        .iter()
        .any(|component| !versions.contains_key(component))
    {
        return Err(format!("{label}.versions is missing a required component"));
    }
    Ok(versions)
}

fn validate_version_token(value: &str, label: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_VERSION_TOKEN_BYTES {
        return Err(format!(
            "{label} version must contain 1 to {MAX_VERSION_TOKEN_BYTES} bytes"
        ));
    }
    if !bytes[0].is_ascii_alphanumeric()
        || bytes[1..].iter().any(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(*byte, b'.' | b'_' | b'+' | b'-')
        })
    {
        return Err(format!("{label} version contains unsafe characters"));
    }
    Ok(())
}
