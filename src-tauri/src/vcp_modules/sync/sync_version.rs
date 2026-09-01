use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

pub(super) const WIRE_PROTOCOL_VERSION: &str = "1.5";
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

#[cfg(test)]
mod tests {
    use super::{
        build_version_check, parse_version_ack, DesktopBackendMode, VersionContractError,
        WIRE_PROTOCOL_VERSION,
    };
    use serde_json::{json, Value};

    fn valid_ack(versions: Value) -> String {
        json!({
            "type": "VERSION_ACK",
            "versions": versions,
            "backendMode": "cds",
        })
        .to_string()
    }

    #[test]
    fn version_check_matches_the_wire_1_5_contract() {
        let frame = build_version_check("1.1.6").expect("valid Mobile version");
        let actual = serde_json::to_value(frame).expect("serialize VERSION_CHECK");
        let fixture: Value =
            serde_json::from_str(include_str!("fixtures/version_handshake_contract.json"))
                .expect("version handshake fixture");
        assert_eq!(actual, fixture["versionCheck"]);
        assert_eq!(fixture["wireVersion"], WIRE_PROTOCOL_VERSION);
    }

    #[test]
    fn version_ack_is_order_independent_and_package_version_is_diagnostic() {
        for versions in [
            json!([
                {"component": "desktop_plugin", "version": "1.5.0"},
                {"component": "wire", "version": "1.5"}
            ]),
            json!([
                {"component": "wire", "version": "1.5"},
                {"component": "desktop_plugin", "version": "9.9.9"}
            ]),
        ] {
            let accepted = parse_version_ack(&valid_ack(versions)).expect("compatible ACK");
            assert_eq!(accepted.wire_version, WIRE_PROTOCOL_VERSION);
            assert_eq!(accepted.backend_mode, DesktopBackendMode::Cds);
        }
    }

    #[test]
    fn version_ack_rejects_invalid_claim_sets_before_comparing_wire() {
        let invalid = [
            json!([{"component": "wire", "version": "1.4"}]),
            json!([
                {"component": "wire", "version": "1.4"},
                {"component": "wire", "version": "1.5"}
            ]),
            json!([
                {"component": "mobile_app", "version": "1.1.6"},
                {"component": "wire", "version": "1.4"}
            ]),
            json!([
                {"component": "desktop_plugin", "version": "1.5.0", "extra": true},
                {"component": "wire", "version": "1.4"}
            ]),
            json!([
                {"component": "desktop_plugin", "version": "bad version"},
                {"component": "wire", "version": "1.4"}
            ]),
            json!([
                {"component": "desktop_plugin", "version": "1".repeat(65)},
                {"component": "wire", "version": "1.4"}
            ]),
            json!([
                {"component": "desktop_plugin", "version": 150},
                {"component": "wire", "version": "1.4"}
            ]),
        ];
        for versions in invalid {
            assert!(matches!(
                parse_version_ack(&valid_ack(versions)),
                Err(VersionContractError::Invalid(_))
            ));
        }
    }

    #[test]
    fn compatible_shape_with_wrong_wire_has_one_mismatch_result() {
        let error = parse_version_ack(&valid_ack(json!([
            {"component": "desktop_plugin", "version": "1.5.9"},
            {"component": "wire", "version": "1.4"}
        ])))
        .expect_err("wire mismatch");
        assert_eq!(
            error,
            VersionContractError::Mismatch {
                expected: WIRE_PROTOCOL_VERSION,
                received: "1.4".to_string(),
                package_version: "1.5.9".to_string(),
            }
        );
    }

    #[test]
    fn old_top_level_fields_and_unsafe_tokens_are_rejected() {
        assert!(parse_version_ack(
            &json!({
                "type": "VERSION_ACK",
                "pluginVersion": "1.5.0",
                "protocolVersion": "1.5",
                "backendMode": "cds"
            })
            .to_string()
        )
        .is_err());
        assert!(build_version_check("bad\nversion").is_err());
        assert!(build_version_check(&"1".repeat(65)).is_err());
    }
}
