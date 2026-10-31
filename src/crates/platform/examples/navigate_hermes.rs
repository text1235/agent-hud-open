//! Exercise the same native destination used by a Hermes completion card.
fn main() {
    let Some(id) = std::env::args().nth(1) else {
        std::process::exit(2)
    };
    let result = agent_hud_platform::NativePlatform::new().and_then(|platform| {
        platform.navigate_session(&serde_json::json!({"kind":"hermes-session","id":id}))
    });
    println!("{}", serde_json::json!({"result":result}));
    if !matches!(result, Ok(true)) {
        std::process::exit(1)
    }
}
