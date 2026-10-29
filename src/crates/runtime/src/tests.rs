use super::*;
use std::{collections::HashMap, fs, path::Path};

#[cfg(unix)]
#[path = "antigravity_cli_tests_unix.rs"]
mod antigravity_cli;
#[cfg(windows)]
#[path = "antigravity_cli_tests_windows.rs"]
mod antigravity_cli;

fn isolated(root: &Path) -> Runtime {
    fs::create_dir_all(root.join("home")).unwrap();
    Runtime::with_sources(Vec::new(), root.join("app-data")).unwrap()
}

fn delegate(runtime: &Runtime, root: &Path) {
    // A regular fixture file satisfies the fixed-owner identity check. It is
    // deliberately never launched; this tests installation and persistence.
    let executable = root.join("fixed hook owner");
    fs::write(&executable, b"fixture only; never execute").unwrap();
    runtime
        .hooks_handle()
        .lock()
        .unwrap()
        .delegate_owner(hooks::InstallContext {
            home: root.join("home"),
            environment: HashMap::new(),
            canonical_executable: Some(executable.clone()),
            executable,
        })
        .unwrap();
}

fn status(runtime: &mut Runtime) -> Value {
    runtime
        .request("antigravity_cli_status", json!({}))
        .unwrap()
}

fn set_paths(runtime: &mut Runtime, value: Value) -> Result<Value, String> {
    runtime.request(
        "set_setting",
        json!({"key":"antigravityCliSettings","value":value}),
    )
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn desktop_questions_preference_is_explicit_persisted_and_does_not_install_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let mut runtime = isolated(temp.path());
    assert!(!runtime.core.codex_desktop_questions_enabled());
    for value in [json!(null), json!("true"), json!(1)] {
        assert!(runtime
            .request("set_setting", json!({"key":"codexDesktopQuestions","value":value}))
            .is_err());
    }
    runtime
        .request("set_setting", json!({"key":"codexDesktopQuestions","value":true}))
        .unwrap();
    assert!(runtime.core.codex_desktop_questions_enabled());
    assert!(!runtime.core.client_hooks_enabled());
    assert!(!runtime.hooks_handle().lock().unwrap().is_listening());
    assert!(!temp.path().join("home/.codex/hooks.json").exists());
    drop(runtime);
    let restored = isolated(temp.path());
    assert!(restored.core.codex_desktop_questions_enabled());
    assert!(!restored.core.client_hooks_enabled());
}

#[test]
fn demo_never_starts_desktop_questions_even_with_saved_preference() {
    let temp = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::demo(temp.path().join("demo"), None).unwrap();
    runtime
        .request("set_setting", json!({"key":"codexDesktopQuestions","value":true}))
        .unwrap();
    runtime.configure_desktop_questions().unwrap();
    let handle = runtime.hooks_handle();
    let hooks = handle.lock().unwrap();
    assert!(!hooks.desktop_questions_enabled());
    assert!(!hooks.is_listening());
}

#[test]
fn antigravity_cli_rejects_invalid_lists_and_missing_owners_without_file_changes() {
    let temp = tempfile::tempdir().unwrap();
    let mut runtime = isolated(temp.path());
    let path = temp.path().join("home/profile/settings.json");
    assert_eq!(status(&mut runtime)["installable"], false);
    assert!(status(&mut runtime)["notice"]
        .as_str()
        .is_some_and(|text| !text.is_empty()));
    assert!(set_paths(&mut runtime, json!([path])).is_err());
    assert!(!path.exists());

    delegate(&runtime, temp.path());
    for invalid in [
        json!(null),
        json!("settings.json"),
        json!([1]),
        json!([""]),
        json!([" /path"]),
        json!(["/path\0invalid"]),
    ] {
        assert!(set_paths(&mut runtime, invalid).is_err());
        assert!(!path.exists());
        assert!(runtime.core.antigravity_cli_settings().is_empty());
        assert!(!temp.path().join("app-data/settings.json").exists());
    }
}
