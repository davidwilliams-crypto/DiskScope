//! Startup restrictions are merged monotonically: command-line flags cannot
//! relax a portable deployment's companion policy.
use serde::Deserialize;
use std::{fs, path::Path};

pub const POLICY_FILE: &str = "diskscope-policy.json";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SessionPolicy {
    pub helpdesk: bool,
    pub no_history: bool,
}

impl SessionPolicy {
    pub fn merge(self, other: Self) -> Self {
        Self {
            helpdesk: self.helpdesk || other.helpdesk,
            no_history: self.no_history || other.no_history,
        }
    }
    pub fn persistence_allowed(self) -> bool {
        !self.helpdesk && !self.no_history
    }
    pub fn file_actions_allowed(self) -> bool {
        !self.helpdesk
    }
    pub fn label(self) -> &'static str {
        if self.helpdesk {
            "Helpdesk diagnostics · no file actions · no profile persistence"
        } else if self.no_history {
            "Session only · no history, preferences or window state saved"
        } else {
            "Local history enabled · paths and metadata are stored on this computer"
        }
    }
    /// Missing policy keeps ordinary interactive behavior. Present but invalid,
    /// unreadable or oversized policy aborts startup rather than relaxing it.
    pub fn beside_executable(executable: &Path) -> Result<Self, String> {
        let directory = executable
            .parent()
            .ok_or("Cannot locate executable directory")?;
        let path = directory.join(POLICY_FILE);
        let mut file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(format!(
                    "Cannot read session policy {}: {error}",
                    path.display()
                ));
            }
        };
        use std::io::Read;
        let mut bytes = Vec::new();
        (&mut file)
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read session policy: {e}"))?;
        if bytes.len() > 4096 {
            return Err("Session policy exceeds 4096 bytes".into());
        }
        let shape: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid session policy {}: {e}", path.display()))?;
        if !shape.is_object() {
            return Err("Session policy must be a JSON object".into());
        }
        // Parse the original bytes again to retain duplicate-field detection.
        serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid session policy {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restrictions_are_monotonic_and_helpdesk_disables_all_persistence() {
        let helpdesk = SessionPolicy {
            helpdesk: true,
            no_history: false,
        };
        let session = SessionPolicy {
            helpdesk: false,
            no_history: true,
        };
        for p in [helpdesk, session, helpdesk.merge(session)] {
            assert!(!p.persistence_allowed());
        }
        assert!(!helpdesk.file_actions_allowed());
        assert!(session.file_actions_allowed());
        assert_eq!(helpdesk.merge(SessionPolicy::default()), helpdesk);
    }
    #[test]
    fn policy_missing_valid_unknown_malformed_and_oversized() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("DiskScope.exe");
        let file = temp.path().join(POLICY_FILE);
        assert_eq!(
            SessionPolicy::beside_executable(&exe).unwrap(),
            SessionPolicy::default()
        );
        fs::write(&file, br#"{"helpdesk":true,"no_history":true}"#).unwrap();
        assert!(SessionPolicy::beside_executable(&exe).unwrap().helpdesk);
        for invalid in [
            b"[]".as_slice(),
            b"[false,false]",
            b"[true]",
            br#"{"helpdesk":true,"unknown":false}"#.as_slice(),
            b"null",
            b"{",
            br#"{"helpdesk":"yes"}"#,
        ] {
            fs::write(&file, invalid).unwrap();
            assert!(SessionPolicy::beside_executable(&exe).is_err());
        }
        fs::write(&file, vec![b' '; 4097]).unwrap();
        assert!(SessionPolicy::beside_executable(&exe).is_err());
    }
}
