//! Vendor-neutral declarations for MCP clients and Rust library hosts.
//!
//! A manifest describes how a downstream tool consumes open-why. It never loads
//! downstream code into the open-why process.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const INTEGRATION_CONTRACT: &str = "open-why.integration/v1";
pub const STORE_IDENTITY_ENV: &str = "OPEN_WHY_STORE_INSTANCE_ID";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntegrationManifest {
    pub contract: String,
    pub integration_id: String,
    pub integration_version: String,
    pub mode: IntegrationMode,
    pub capabilities: Vec<IntegrationCapability>,
    pub scope: ScopeDeclaration,
    pub store_identity: StoreIdentityDeclaration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpDeclaration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<RustDeclaration>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrationMode {
    McpStdio,
    RustLibrary,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum IntegrationCapability {
    Ask,
    Index,
    Capture,
    Import,
    Search,
    Current,
    History,
    CommitLinks,
    Link,
    Feedback,
}

impl IntegrationCapability {
    pub fn mcp_tool(self) -> &'static str {
        match self {
            Self::Ask => "open-why_ask",
            Self::Index => "open-why_index",
            Self::Capture => "open-why_capture",
            Self::Import => "open-why_import",
            Self::Search => "open-why_search",
            Self::Current => "open-why_get",
            Self::History => "open-why_history",
            Self::CommitLinks => "open-why_commit_links",
            Self::Link => "open-why_link",
            Self::Feedback => "open-why_feedback",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopeDeclaration {
    pub strategy: ScopeStrategy,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ScopeStrategy {
    Explicit,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoreIdentityDeclaration {
    pub strategy: StoreIdentityStrategy,
    pub environment: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum StoreIdentityStrategy {
    PerInstallation,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpDeclaration {
    pub command: String,
    pub args: Vec<String>,
    pub protocol_version: String,
    pub contracts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RustDeclaration {
    pub crate_name: String,
    pub minimum_version: String,
    pub contracts: Vec<String>,
}

impl IntegrationManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.contract != INTEGRATION_CONTRACT {
            return Err(format!("contract must be `{INTEGRATION_CONTRACT}`"));
        }
        validate_identifier(&self.integration_id)?;
        validate_version(&self.integration_version, "integration_version")?;
        if self.capabilities.is_empty() {
            return Err("capabilities must not be empty".to_owned());
        }
        let unique: HashSet<_> = self.capabilities.iter().collect();
        if unique.len() != self.capabilities.len() {
            return Err("capabilities must not contain duplicates".to_owned());
        }
        if self.store_identity.strategy != StoreIdentityStrategy::PerInstallation
            || self.store_identity.environment != STORE_IDENTITY_ENV
        {
            return Err(format!(
                "store identity must be per-installation through `{STORE_IDENTITY_ENV}`"
            ));
        }
        match self.mode {
            IntegrationMode::McpStdio => {
                let mcp = self
                    .mcp
                    .as_ref()
                    .ok_or_else(|| "mcp is required for mcp-stdio mode".to_owned())?;
                if self.rust.is_some() {
                    return Err("rust must be absent for mcp-stdio mode".to_owned());
                }
                validate_mcp(mcp)?;
            }
            IntegrationMode::RustLibrary => {
                let rust = self
                    .rust
                    .as_ref()
                    .ok_or_else(|| "rust is required for rust-library mode".to_owned())?;
                if self.mcp.is_some() {
                    return Err("mcp must be absent for rust-library mode".to_owned());
                }
                validate_rust(rust)?;
            }
        }
        Ok(())
    }
}

fn validate_identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err("integration_id must be 1..128 ASCII identifier characters".to_owned());
    }
    Ok(())
}

fn validate_version(value: &str, field: &str) -> Result<(), String> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(format!(
            "{field} must be a numeric major.minor.patch version"
        ));
    }
    Ok(())
}

fn validate_contracts(contracts: &[String]) -> Result<(), String> {
    if contracts.is_empty() {
        return Err("contracts must not be empty".to_owned());
    }
    if contracts
        .iter()
        .any(|contract| !valid_contract_name(contract))
    {
        return Err("contracts must be bounded, versioned open-why contract names".to_owned());
    }
    let unique: HashSet<_> = contracts.iter().collect();
    if unique.len() != contracts.len() {
        return Err("contracts must not contain duplicates".to_owned());
    }
    Ok(())
}

fn valid_contract_name(contract: &str) -> bool {
    if contract.is_empty() || contract.len() > 128 {
        return false;
    }
    let Some(remainder) = contract.strip_prefix("open-why.") else {
        return false;
    };
    let Some((name, version)) = remainder.rsplit_once("/v") else {
        return false;
    };
    !name.is_empty() && !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_mcp(mcp: &McpDeclaration) -> Result<(), String> {
    if mcp.command.is_empty() || mcp.command.len() > 1024 || mcp.args.len() > 32 {
        return Err("mcp command or args exceed integration bounds".to_owned());
    }
    if mcp.args.iter().any(|arg| arg.len() > 1024) {
        return Err("mcp args exceed integration bounds".to_owned());
    }
    if mcp.protocol_version.is_empty() || mcp.protocol_version.len() > 32 {
        return Err("mcp protocol_version is invalid".to_owned());
    }
    validate_contracts(&mcp.contracts)
}

fn validate_rust(rust: &RustDeclaration) -> Result<(), String> {
    if rust.crate_name != "open-why" {
        return Err("rust crate_name must be `open-why`".to_owned());
    }
    validate_version(&rust.minimum_version, "rust minimum_version")?;
    validate_contracts(&rust.contracts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp_manifest() -> IntegrationManifest {
        IntegrationManifest {
            contract: INTEGRATION_CONTRACT.to_owned(),
            integration_id: "dev.example.agent".to_owned(),
            integration_version: "1.0.0".to_owned(),
            mode: IntegrationMode::McpStdio,
            capabilities: vec![IntegrationCapability::Ask, IntegrationCapability::Current],
            scope: ScopeDeclaration {
                strategy: ScopeStrategy::Explicit,
            },
            store_identity: StoreIdentityDeclaration {
                strategy: StoreIdentityStrategy::PerInstallation,
                environment: STORE_IDENTITY_ENV.to_owned(),
            },
            mcp: Some(McpDeclaration {
                command: "why".to_owned(),
                args: vec!["serve".to_owned()],
                protocol_version: "2024-11-05".to_owned(),
                contracts: vec!["open-why.current-rationale/v1".to_owned()],
            }),
            rust: None,
        }
    }

    #[test]
    fn valid_mcp_manifest_passes() {
        assert_eq!(mcp_manifest().validate(), Ok(()));
    }

    #[test]
    fn mode_specific_configuration_is_exclusive() {
        let mut manifest = mcp_manifest();
        manifest.rust = Some(RustDeclaration {
            crate_name: "open-why".to_owned(),
            minimum_version: "0.1.0".to_owned(),
            contracts: vec!["open-why.current-rationale/v1".to_owned()],
        });
        assert_eq!(
            manifest.validate(),
            Err("rust must be absent for mcp-stdio mode".to_owned())
        );
    }

    #[test]
    fn duplicate_capabilities_fail() {
        let mut manifest = mcp_manifest();
        manifest.capabilities.push(IntegrationCapability::Ask);
        assert_eq!(
            manifest.validate(),
            Err("capabilities must not contain duplicates".to_owned())
        );
    }

    #[test]
    fn store_identity_is_fixed_to_provider_minted_environment() {
        let mut manifest = mcp_manifest();
        manifest.store_identity.environment = "HOME".to_owned();
        assert!(manifest
            .validate()
            .unwrap_err()
            .contains(STORE_IDENTITY_ENV));
    }

    #[test]
    fn contract_names_end_in_a_numeric_version() {
        for invalid in [
            "open-why.thing/v1abc",
            "open-why.thing/vX/extra",
            "open-why.thing/v",
            "other.thing/v1",
        ] {
            let mut manifest = mcp_manifest();
            manifest.mcp.as_mut().unwrap().contracts = vec![invalid.to_owned()];
            assert!(manifest.validate().is_err(), "accepted {invalid}");
        }
    }
}
