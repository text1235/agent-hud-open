//! Native destinations derived from the session's own persisted identity.
use serde_json::{json, Value};

pub(crate) fn hermes_target(id: &str) -> Option<Value> {
    let id = id.strip_prefix("hermes:")?;
    // A named profile requires a verified profile-specific desktop route.
    // Keep its identity intact instead of opening it in the default profile.
    if id.is_empty() || id.contains(':') {
        return None;
    }
    Some(json!({"kind":"hermes-session","id":id}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_session_identity_becomes_a_native_destination() {
        assert_eq!(
            hermes_target("hermes:20260101_000000_example").unwrap()["id"],
            "20260101_000000_example"
        );
        assert!(hermes_target("codex:20260101_000000_example").is_none());
        assert!(hermes_target("hermes:").is_none());
        assert!(hermes_target("hermes:other-profile:20260101_000000_example").is_none());
    }
}
