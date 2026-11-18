use anyhow::Context;
use serde::Deserialize;
use serde_json::Value;

const TABLE: &str = "breaking-bump";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct Settings {
    #[serde(default = "enabled")]
    pub(crate) require_marker: bool,

    #[serde(default = "enabled")]
    pub(crate) deny_inherited: bool,
}

impl Settings {
    pub(crate) fn from_workspace_metadata(metadata: &Value) -> anyhow::Result<Self> {
        let table = metadata
            .get(TABLE)
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()));
        serde_json::from_value(table)
            .with_context(|| format!("invalid settings in [workspace.metadata.{TABLE}]"))
    }
}

fn enabled() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use crate::domain::settings::Settings;
    use serde_json::json;

    #[test]
    fn defaults_are_enabled() {
        for metadata in [
            json!(null),
            json!({}),
            json!({ "other": 1 }),
            json!({ "breaking-bump": {} }),
        ] {
            assert_eq!(
                Settings::from_workspace_metadata(&metadata).unwrap(),
                Settings {
                    require_marker: true,
                    deny_inherited: true
                }
            );
        }
    }

    #[test]
    fn explicit_settings() {
        let metadata =
            json!({ "breaking-bump": { "require-marker": false, "deny-inherited": false } });
        assert_eq!(
            Settings::from_workspace_metadata(&metadata).unwrap(),
            Settings {
                require_marker: false,
                deny_inherited: false
            }
        );
    }

    #[test]
    fn unknown_keys_are_an_error() {
        let metadata = json!({ "breaking-bump": { "require-markers": false } });
        assert!(Settings::from_workspace_metadata(&metadata).is_err());
    }
}
