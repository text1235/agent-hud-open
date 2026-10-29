#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod workbench;

fn main() {
    let demo = std::env::args().any(|arg| matches!(arg.as_str(), "--demo" | "--demo-script"));
    let extension = if demo { None } else {
        Some(std::sync::Arc::new(workbench::Workbench::default()) as std::sync::Arc<dyn agent_hud_host::extension::ProductExtension>)
    };
    agent_hud_host::run_with_context(tauri::generate_context!(), extension);
}
