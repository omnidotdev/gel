use serde::{Deserialize, Serialize};

/// A file whose full content is managed declaratively
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedFile {
    /// Absolute path the file should live at
    pub path: String,
    /// Exact content the file should hold
    pub content: String,
}

/// Explicit intent over systemd units, expressed as two disjoint lists
///
/// This is deliberately NOT full-set convergence: only the units named here are
/// ever touched by apply. Units absent from both lists are left exactly as they
/// are, so gel never disables a unit it was not told about.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceIntent {
    /// Units to ensure are enabled
    #[serde(default)]
    pub enable: Vec<String>,
    /// Units to ensure are disabled
    #[serde(default)]
    pub disable: Vec<String>,
}

/// Explicit packages and managed files a machine should have
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredState {
    /// Explicitly installed native (official-repo) packages
    pub native: Vec<String>,
    /// Explicitly installed foreign (AUR) packages
    pub foreign: Vec<String>,
    /// Files whose content is managed declaratively
    ///
    /// Defaulted so phase-1 artifacts without a `files` key still deserialize
    #[serde(default)]
    pub files: Vec<ManagedFile>,
    /// Explicit enable/disable intent over systemd units
    ///
    /// Defaulted so artifacts predating the service model still deserialize
    #[serde(default)]
    pub services: ServiceIntent,
}

/// Actual explicit packages currently on the machine
pub type SystemState = DesiredState;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desired_state_json_roundtrips() {
        let state = DesiredState {
            native: vec!["git".to_owned(), "vim".to_owned()],
            foreign: vec!["yay".to_owned()],
            files: vec![ManagedFile {
                path: "/etc/hostname".to_owned(),
                content: "gelbox\n".to_owned(),
            }],
            services: ServiceIntent {
                enable: vec!["sshd.service".to_owned()],
                disable: vec!["bluetooth.service".to_owned()],
            },
        };

        let json = serde_json::to_string(&state).expect("serialize");
        let back: DesiredState = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(state, back);
    }

    #[test]
    fn json_without_files_deserializes_to_empty() {
        // A phase-1 artifact carries no `files` key and must still deserialize
        let json = r#"{"native":["git"],"foreign":[]}"#;

        let state: DesiredState = serde_json::from_str(json).expect("deserialize");

        assert!(state.files.is_empty());
        assert_eq!(state.native, vec!["git".to_owned()]);
    }

    #[test]
    fn json_without_services_deserializes_to_empty() {
        // An artifact predating the service model carries no `services` key and
        // must still deserialize with empty enable/disable intent
        let json = r#"{"native":["git"],"foreign":[],"files":[]}"#;

        let state: DesiredState = serde_json::from_str(json).expect("deserialize");

        assert!(state.services.enable.is_empty());
        assert!(state.services.disable.is_empty());
    }
}
