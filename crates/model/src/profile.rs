//! Layout profile configuration (report §5.3, §13.2).
//!
//! Persisted as JSON with an explicit `schema_version`. Loading a file with a
//! newer schema than we understand must fail without touching the file.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutProfile {
    pub schema_version: u32,
    pub profile_id: String,
    pub revision: u64,
    pub name: String,
    #[serde(default)]
    pub environment: Environment,
    #[serde(default)]
    pub placements: Vec<Placement>,
    #[serde(default)]
    pub switch_policy: SwitchPolicy,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    #[serde(default)]
    pub required_endpoints: Vec<String>,
    #[serde(default)]
    pub optional_endpoints: Vec<String>,
}

/// Where one device's display sits in the shared layout, in layout units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub endpoint: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SwitchPolicy {
    pub edge_enabled: bool,
    pub wrap_horizontal: bool,
    pub block_fullscreen: bool,
    pub block_maximized: bool,
    pub block_corner_dip: u32,
    pub excluded_apps: Vec<String>,
    pub double_hit_override: bool,
}

impl Default for SwitchPolicy {
    fn default() -> Self {
        Self {
            edge_enabled: true,
            wrap_horizontal: false,
            block_fullscreen: false,
            block_maximized: false,
            block_corner_dip: 16,
            excluded_apps: Vec::new(),
            double_hit_override: false,
        }
    }
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile schema {found} is newer than supported {supported}; refusing to load")]
    UnsupportedSchema { found: u32, supported: u32 },
}

impl LayoutProfile {
    /// Rejects profiles written by a newer build instead of silently dropping
    /// fields we do not understand.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema_version > PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::UnsupportedSchema {
                found: self.schema_version,
                supported: PROFILE_SCHEMA_VERSION,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "schema_version": 1,
        "profile_id": "office-desk",
        "revision": 7,
        "name": "办公室",
        "environment": {
            "required_endpoints": ["mac-a/display-main", "pc-b/display-main"],
            "optional_endpoints": ["mac-a/display-side"]
        },
        "placements": [
            {"endpoint": "mac-a/display-main", "x": 0, "y": 0, "w": 1440, "h": 900},
            {"endpoint": "pc-b/display-main", "x": 1440, "y": 0, "w": 1920, "h": 1080}
        ],
        "switch_policy": {"edge_enabled": true, "block_corner_dip": 16}
    }"#;

    #[test]
    fn parses_report_example() {
        let p: LayoutProfile = serde_json::from_str(SAMPLE).unwrap();
        p.validate().unwrap();
        assert_eq!(p.placements.len(), 2);
        assert!(!p.switch_policy.wrap_horizontal);
    }

    #[test]
    fn rejects_newer_schema() {
        let mut p: LayoutProfile = serde_json::from_str(SAMPLE).unwrap();
        p.schema_version = PROFILE_SCHEMA_VERSION + 1;
        assert!(matches!(
            p.validate(),
            Err(ProfileError::UnsupportedSchema { .. })
        ));
    }
}
