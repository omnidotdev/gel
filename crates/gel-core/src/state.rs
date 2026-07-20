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

/// A single global system setting gel can manage declaratively
///
/// Deriving [`Hash`] and [`Eq`] lets a key index a map, which the in-memory test
/// backend relies on to store per-setting values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SettingKey {
    /// The machine hostname
    Hostname,
    /// The system timezone
    Timezone,
    /// The system locale
    Locale,
}

/// Declarative intent over global system settings
///
/// Every field is optional: an unset setting is one gel does not manage and
/// never touches, mirroring the explicit-intent posture of [`ServiceIntent`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsIntent {
    /// Desired hostname, or `None` to leave it unmanaged
    #[serde(default)]
    pub hostname: Option<String>,
    /// Desired timezone, or `None` to leave it unmanaged
    #[serde(default)]
    pub timezone: Option<String>,
    /// Desired locale, or `None` to leave it unmanaged
    #[serde(default)]
    pub locale: Option<String>,
}

impl SettingsIntent {
    /// Return the declared settings as `(key, value)` pairs
    ///
    /// Only settings set to `Some` are returned, in the deterministic order
    /// Hostname, Timezone, Locale, so planning over them is reproducible.
    #[must_use]
    pub fn declared(&self) -> Vec<(SettingKey, String)> {
        let mut out = Vec::new();
        if let Some(hostname) = &self.hostname {
            out.push((SettingKey::Hostname, hostname.clone()));
        }
        if let Some(timezone) = &self.timezone {
            out.push((SettingKey::Timezone, timezone.clone()));
        }
        if let Some(locale) = &self.locale {
            out.push((SettingKey::Locale, locale.clone()));
        }
        out
    }
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
    /// Declarative intent over global system settings
    ///
    /// Defaulted so artifacts predating the settings model still deserialize
    #[serde(default)]
    pub settings: SettingsIntent,
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
            settings: SettingsIntent::default(),
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

    #[test]
    fn json_without_settings_deserializes_to_all_none() {
        // An artifact predating the settings model carries no `settings` key and
        // must still deserialize with every setting unset
        let json = r#"{"native":["git"],"foreign":[],"files":[]}"#;

        let state: DesiredState = serde_json::from_str(json).expect("deserialize");

        assert_eq!(state.settings, SettingsIntent::default());
        assert!(state.settings.hostname.is_none());
        assert!(state.settings.timezone.is_none());
        assert!(state.settings.locale.is_none());
    }

    #[test]
    fn desired_state_roundtrips_with_settings() {
        let state = DesiredState {
            native: vec![],
            foreign: vec![],
            files: vec![],
            services: ServiceIntent::default(),
            settings: SettingsIntent {
                hostname: Some("gelbox".to_owned()),
                timezone: Some("UTC".to_owned()),
                locale: Some("en_US.UTF-8".to_owned()),
            },
        };

        let json = serde_json::to_string(&state).expect("serialize");
        let back: DesiredState = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(state, back);
    }

    #[test]
    fn declared_returns_only_set_values_in_fixed_order() {
        // Only Some settings are returned, in Hostname, Timezone, Locale order
        let settings = SettingsIntent {
            hostname: Some("gelbox".to_owned()),
            timezone: None,
            locale: Some("en_US.UTF-8".to_owned()),
        };

        assert_eq!(
            settings.declared(),
            vec![
                (SettingKey::Hostname, "gelbox".to_owned()),
                (SettingKey::Locale, "en_US.UTF-8".to_owned()),
            ]
        );
    }

    #[test]
    fn declared_is_empty_when_nothing_is_set() {
        assert!(SettingsIntent::default().declared().is_empty());
    }

    #[test]
    fn declared_preserves_hostname_timezone_locale_order() {
        let settings = SettingsIntent {
            hostname: Some("h".to_owned()),
            timezone: Some("t".to_owned()),
            locale: Some("l".to_owned()),
        };

        let keys: Vec<SettingKey> = settings.declared().into_iter().map(|(k, _)| k).collect();

        assert_eq!(
            keys,
            vec![
                SettingKey::Hostname,
                SettingKey::Timezone,
                SettingKey::Locale
            ]
        );
    }
}
