use std::{collections::HashSet, fs, io::ErrorKind, path::Path};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub role: String,
    pub created: String,
    pub bio: String,
    pub skills: Vec<String>,
    pub color: String,
    pub avatar: String,
    pub avatar_file: Option<String>,
}

pub fn normalize_agent_name(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn load_profile_allowlist(path: &Path) -> HashSet<String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return HashSet::new(),
        Err(error) => {
            tracing::warn!(path = %path.display(), "failed to read profiles allowlist: {error}");
            return HashSet::new();
        }
    };

    let profiles = match serde_json::from_str::<Vec<AgentProfile>>(&raw) {
        Ok(profiles) => profiles,
        Err(error) => {
            tracing::warn!(path = %path.display(), "failed to parse profiles allowlist json: {error}");
            return HashSet::new();
        }
    };

    profiles
        .into_iter()
        .map(|profile| normalize_agent_name(&profile.name))
        .filter(|name| !name.is_empty())
        .collect::<HashSet<String>>()
}

pub fn load_hermes_profile_allowlist() -> HashSet<String> {
    let home = match std::env::var("HOME") {
        Ok(home) if !home.trim().is_empty() => home,
        _ => return HashSet::new(),
    };

    load_hermes_profile_allowlist_from(Path::new(&home).join(".hermes").join("profiles").as_path())
}

pub fn load_hermes_admission_allowlist() -> HashSet<String> {
    let mut allowed = load_hermes_profile_allowlist();
    if allowed.is_empty() {
        allowed.insert("hermes".to_string());
    }
    allowed
}

pub fn load_hermes_admission_allowlist_from(profiles_root: &Path) -> HashSet<String> {
    let mut allowed = load_hermes_profile_allowlist_from(profiles_root);
    if allowed.is_empty() {
        allowed.insert("hermes".to_string());
    }
    allowed
}

pub fn load_hermes_profile_allowlist_from(profiles_root: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    let entries = match fs::read_dir(profiles_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return out,
        Err(error) => {
            tracing::warn!(path = %profiles_root.display(), "failed to read hermes profiles root: {error}");
            return out;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::debug!(path = %profiles_root.display(), "failed to read hermes profile entry: {error}");
                continue;
            }
        };

        let is_dir = match entry.file_type() {
            Ok(kind) => kind.is_dir(),
            Err(error) => {
                tracing::debug!(path = %entry.path().display(), "failed to inspect hermes profile entry type: {error}");
                continue;
            }
        };
        if !is_dir {
            continue;
        }

        // Treat only real profile directories as admissible identities.
        if !entry.path().join("config.yaml").exists() {
            continue;
        }

        let name = normalize_agent_name(&entry.file_name().to_string_lossy());
        if !name.is_empty() {
            out.insert(name);
        }
    }

    out
}

pub fn load_local_admission_allowlist(relay_profiles_path: &Path) -> HashSet<String> {
    let mut allowed = load_profile_allowlist(relay_profiles_path);
    allowed.extend(load_hermes_admission_allowlist());
    allowed
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn hermes_profile_allowlist_reads_directory_identities() {
        let root =
            std::env::temp_dir().join(format!("relay-hermes-profiles-test-{}", std::process::id()));
        let profiles_root = root.join("profiles");
        assert!(fs::create_dir_all(profiles_root.join("Tracie")).is_ok());
        assert!(fs::create_dir_all(profiles_root.join("spoof")).is_ok());
        assert!(fs::create_dir_all(profiles_root.join("not-a-profile")).is_ok());
        assert!(
            fs::write(
                profiles_root.join("Tracie").join("config.yaml"),
                "model: {}\n"
            )
            .is_ok()
        );
        assert!(
            fs::write(
                profiles_root.join("spoof").join("config.yaml"),
                "model: {}\n"
            )
            .is_ok()
        );

        let allowlist = super::load_hermes_profile_allowlist_from(&profiles_root);
        assert!(allowlist.contains("tracie"));
        assert!(allowlist.contains("spoof"));
        assert!(!allowlist.contains("not-a-profile"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn profile_json_allowlist_normalizes_names() {
        let root =
            std::env::temp_dir().join(format!("relay-profiles-json-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let profiles_path = root.join("profiles.json");
        let payload = r#"[
            {"name":"Hermes","role":"coordinator","created":"2026-01-01","bio":"","skills":[],"color":"cyan","avatar":"default","avatar_file":null},
            {"name":" Codex ","role":"coder","created":"2026-01-01","bio":"","skills":[],"color":"green","avatar":"default","avatar_file":null}
        ]"#;
        assert!(fs::write(&profiles_path, payload).is_ok());

        let allowlist = super::load_profile_allowlist(&profiles_path);
        assert!(allowlist.contains("hermes"));
        assert!(allowlist.contains("codex"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hermes_admission_allowlist_falls_back_to_hermes_when_empty() {
        let root = std::env::temp_dir().join(format!(
            "relay-hermes-admission-fallback-test-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&root);

        let allowlist = super::load_hermes_admission_allowlist_from(&root);
        assert_eq!(allowlist.len(), 1);
        assert!(allowlist.contains("hermes"));

        let _ = fs::remove_dir_all(root);
    }
}
