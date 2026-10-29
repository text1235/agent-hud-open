use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "agent-hud-auto-dispatch-{}-{}-{}",
            std::process::id(),
            unique(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn enabling_preserves_original_bytes_is_idempotent_and_has_a_backup() {
    let project = Project::new();
    let file = project.0.join("AGENTS.md");
    let original = "\u{feff}# Existing instructions\r\nPreserve my project rules.\r\n";
    fs::write(&file, original).unwrap();
    assert_eq!(status(&project.0).unwrap()["enabled"], false);
    assert_eq!(enable(&project.0, true).unwrap()["enabled"], true);
    let enabled = fs::read_to_string(&file).unwrap();
    assert_eq!(enabled, format!("{original}{BLOCK}"));
    let folder = project.0.join(".agent-hud/backup");
    let backups: Vec<_> = fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), original);
    enable(&project.0, true).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), enabled);
    assert_eq!(fs::read_dir(folder).unwrap().count(), 1);
}

#[test]
fn disabling_removes_only_the_exact_owned_block_and_keeps_later_edits() {
    let project = Project::new();
    let file = project.0.join("AGENTS.md");
    let original = "# User rules\n";
    fs::write(&file, original).unwrap();
    enable(&project.0, true).unwrap();
    let later = "\n## Later user rule\nKeep this too.\n";
    fs::write(&file, format!("{original}{BLOCK}{later}")).unwrap();
    assert_eq!(enable(&project.0, false).unwrap()["enabled"], false);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!("{original}{later}")
    );
    assert!(project.0.join(".agent-hud/automatic-dispatch.md").is_file());
    enable(&project.0, false).unwrap();
    assert_eq!(
        fs::read_to_string(file).unwrap(),
        format!("{original}{later}")
    );
}

#[test]
fn missing_agents_is_created_and_disabled_without_touching_other_files() {
    let project = Project::new();
    let keep = project.0.join("keep.txt");
    fs::write(&keep, "unrelated").unwrap();
    enable(&project.0, false).unwrap();
    assert!(!project.0.join("AGENTS.md").exists());
    assert!(!project.0.join(".agent-hud").exists());
    enable(&project.0, true).unwrap();
    let instructions =
        fs::read_to_string(project.0.join(".agent-hud/automatic-dispatch.md")).unwrap();
    assert!(!instructions.contains("{{PROJECT_ROOT}}"));
    assert!(instructions.contains("gemini-4-argon-high"));
    enable(&project.0, false).unwrap();
    assert_eq!(fs::read_to_string(project.0.join("AGENTS.md")).unwrap(), "");
    assert_eq!(fs::read_to_string(keep).unwrap(), "unrelated");
}

#[test]
fn malformed_or_edited_blocks_are_preserved_and_refused() {
    let project = Project::new();
    let file = project.0.join("AGENTS.md");
    for malformed in [
        START.to_string(),
        END.to_string(),
        format!("{START}\nmodified\n{END}"),
        format!("{BLOCK}{BLOCK}"),
    ] {
        fs::write(&file, &malformed).unwrap();
        assert!(enable(&project.0, true).is_err());
        assert!(enable(&project.0, false).is_err());
        assert!(status(&project.0).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), malformed);
    }
    assert!(!project.0.join(".agent-hud").exists());
}

#[test]
fn roots_must_be_absolute_existing_directories() {
    assert!(enable(Path::new("relative-project"), true).is_err());
    assert!(status(Path::new("relative-project")).is_err());
    let project = Project::new();
    assert!(enable(&project.0.join("missing"), true).is_err());
    let file = project.0.join("file.txt");
    fs::write(&file, "file").unwrap();
    assert!(status(&file).is_err());
}
