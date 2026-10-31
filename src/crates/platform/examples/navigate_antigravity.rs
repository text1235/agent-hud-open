//! Explicit local navigation diagnostic; does not send a message to a model.
fn main() {
    let Some(id) = std::env::args().nth(1) else {
        eprintln!("Pass the exact Antigravity conversation UUID");
        std::process::exit(2);
    };
    let result = agent_hud_platform::NativePlatform::new().and_then(|platform| {
        platform.navigate_session(&serde_json::json!({
            "kind":"antigravity-conversation", "id":id
        }))
    });
    println!("{}", serde_json::json!({"result":result}));
    if !matches!(result, Ok(true)) {
        std::process::exit(1);
    }
}
