use serde::{Deserialize, Serialize};

/// Explicit packages a machine should have, split by origin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesiredState {
    /// Explicitly installed native (official-repo) packages
    pub native: Vec<String>,
    /// Explicitly installed foreign (AUR) packages
    pub foreign: Vec<String>,
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
        };

        let json = serde_json::to_string(&state).expect("serialize");
        let back: DesiredState = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(state, back);
    }
}
