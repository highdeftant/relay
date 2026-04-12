use std::{
    collections::HashMap,
    fs,
    path::Path,
    time::SystemTime,
};

use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct HermesSnapshot {
    pub root_exists: bool,
    pub skills_root_exists: bool,
    pub skill_count: usize,
    pub skill_categories: Vec<String>,
    pub profile_skill_counts: HashMap<String, usize>,
    pub session_count: usize,
    pub recent_sessions: Vec<String>,
    pub state_db_exists: bool,
    pub state_db_bytes: u64,
    pub honcho_hosts: usize,
    pub config_exists: bool,
    pub auth_exists: bool,
    pub processes_file_exists: bool,
    pub known_process_count: usize,
}

pub fn load_snapshot() -> HermesSnapshot {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return HermesSnapshot::default();
    }
    load_snapshot_from(Path::new(&home).join(".hermes").as_path())
}

pub fn load_snapshot_from(root: &Path) -> HermesSnapshot {
    let mut snapshot = HermesSnapshot {
        root_exists: root.exists(),
        ..HermesSnapshot::default()
    };

    let skills_root = root.join("skills");
    snapshot.skills_root_exists = skills_root.exists();
    snapshot.skill_count = count_skill_files(&skills_root);
    snapshot.skill_categories = read_skill_categories(&skills_root);
    snapshot.profile_skill_counts = read_profile_skill_counts(&root.join("profiles"));

    let sessions_root = root.join("sessions");
    let mut sessions = read_sessions(&sessions_root);
    snapshot.session_count = sessions.len();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    snapshot.recent_sessions = sessions.into_iter().take(5).map(|s| s.name).collect();

    let state_db = root.join("state.db");
    if let Ok(meta) = fs::metadata(&state_db) {
        snapshot.state_db_exists = true;
        snapshot.state_db_bytes = meta.len();
    }

    snapshot.honcho_hosts = read_honcho_host_count(&root.join("honcho.json"));
    snapshot.config_exists = root.join("config.yaml").exists();
    snapshot.auth_exists = root.join("auth.json").exists();

    let processes_file = root.join("processes.json");
    snapshot.processes_file_exists = processes_file.exists();
    snapshot.known_process_count = read_process_count(&processes_file);

    snapshot
}

#[derive(Debug)]
struct SessionEntry {
    name: String,
    modified: SystemTime,
}

fn count_skill_files(skills_root: &Path) -> usize {
    count_skill_files_recursive(skills_root).unwrap_or(0)
}

fn count_skill_files_recursive(path: &Path) -> std::io::Result<usize> {
    if !path.exists() {
        return Ok(0);
    }

    let mut total = 0usize;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();

        if entry.file_type()?.is_dir() {
            total = total.saturating_add(count_skill_files_recursive(&entry_path)?);
            continue;
        }

        if entry.file_name().to_string_lossy() == "SKILL.md" {
            total = total.saturating_add(1);
        }
    }

    Ok(total)
}

fn read_skill_categories(skills_root: &Path) -> Vec<String> {
    if !skills_root.exists() {
        return Vec::new();
    }

    let mut categories = Vec::new();
    let entries = match fs::read_dir(skills_root) {
        Ok(rows) => rows,
        Err(_) => return categories,
    };

    for entry in entries.flatten() {
        let is_dir = match entry.file_type() {
            Ok(kind) => kind.is_dir(),
            Err(_) => false,
        };

        if !is_dir {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        if name != "hermes-agent" {
            categories.push(name);
        }
    }

    categories.sort();
    categories
}

fn read_profile_skill_counts(profiles_root: &Path) -> HashMap<String, usize> {
    if !profiles_root.exists() {
        return HashMap::new();
    }

    let mut counts = HashMap::new();
    let rows = match fs::read_dir(profiles_root) {
        Ok(rows) => rows,
        Err(_) => return counts,
    };

    for row in rows.flatten() {
        let is_dir = match row.file_type() {
            Ok(kind) => kind.is_dir(),
            Err(_) => false,
        };
        if !is_dir {
            continue;
        }

        let profile_name = row.file_name().to_string_lossy().to_string();
        let skills_root = row.path().join("skills");
        let skill_count = count_skill_files(&skills_root);
        if skill_count > 0 {
            counts.insert(profile_name, skill_count);
        }
    }

    counts
}

fn read_sessions(sessions_root: &Path) -> Vec<SessionEntry> {
    if !sessions_root.exists() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let rows = match fs::read_dir(sessions_root) {
        Ok(rows) => rows,
        Err(_) => return out,
    };

    for row in rows.flatten() {
        let path = row.path();
        if !path.is_file() {
            continue;
        }

        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if ext != "json" && ext != "jsonl" {
            continue;
        }

        let name = match path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => continue,
        };

        let modified = match fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(ts) => ts,
            Err(_) => SystemTime::UNIX_EPOCH,
        };

        out.push(SessionEntry { name, modified });
    }

    out
}

fn read_honcho_host_count(path: &Path) -> usize {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return 0,
    };

    let value = match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value,
        Err(_) => return 0,
    };

    match value.get("hosts") {
        Some(Value::Object(map)) => map.len(),
        _ => 0,
    }
}

fn read_process_count(path: &Path) -> usize {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => return 0,
    };

    let value = match serde_json::from_str::<Value>(&raw) {
        Ok(value) => value,
        Err(_) => return 0,
    };

    match value {
        Value::Array(items) => items.len(),
        Value::Object(map) => map.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    fn unique_temp_dir() -> PathBuf {
        let base = std::env::temp_dir();
        let nanos = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos(),
            Err(_) => 0,
        };
        base.join(format!("relay-hermes-test-{nanos}"))
    }

    #[test]
    fn snapshot_counts_skills_sessions_and_config() {
        let root = unique_temp_dir();
        let skills_dir = root.join("skills/software-development/demo");
        let sessions_dir = root.join("sessions");

        assert!(fs::create_dir_all(&skills_dir).is_ok());
        assert!(fs::create_dir_all(&sessions_dir).is_ok());
        assert!(fs::write(skills_dir.join("SKILL.md"), "# demo").is_ok());
        assert!(fs::write(sessions_dir.join("session_1.json"), "{}").is_ok());
        assert!(fs::write(root.join("config.yaml"), "default_model: test").is_ok());

        let snapshot = super::load_snapshot_from(&root);

        assert_eq!(snapshot.skill_count, 1);
        assert_eq!(snapshot.session_count, 1);
        assert!(snapshot.config_exists);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_reads_honcho_and_process_counts() {
        let root = unique_temp_dir();
        assert!(fs::create_dir_all(&root).is_ok());
        assert!(
            fs::write(
                root.join("honcho.json"),
                r#"{"hosts": {"hermes.one": {}, "hermes.two": {}}}"#,
            )
            .is_ok()
        );
        assert!(
            fs::write(root.join("processes.json"), r#"[{"name":"relay"},{"name":"hermes"}]"#)
                .is_ok()
        );

        let snapshot = super::load_snapshot_from(&root);

        assert_eq!(snapshot.honcho_hosts, 2);
        assert_eq!(snapshot.known_process_count, 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_counts_profile_skills() {
        let root = unique_temp_dir();
        let skill_path = root.join("profiles/codex/skills/custom/demo/SKILL.md");
        assert!(fs::create_dir_all(skill_path.parent().unwrap_or(&root)).is_ok());
        assert!(fs::write(&skill_path, "# demo").is_ok());

        let snapshot = super::load_snapshot_from(&root);

        assert_eq!(snapshot.profile_skill_counts.get("codex"), Some(&1usize));

        let _ = fs::remove_dir_all(root);
    }
}
