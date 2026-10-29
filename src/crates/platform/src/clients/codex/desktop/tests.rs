use super::*;
use agent_hud_agents::codex::AGENT;
use serde_json::json;

const THREAD: &str = "00000000-0000-4000-8000-000000000001";
fn permission(connector: &str, persist: Value) -> Value {
    json!({"id":7,"method":"mcpServer/elicitation/request","params":{"threadId":THREAD,"turnId":"turn-fixture","mode":"form","_meta":{"codex_approval_kind":"mcp_tool_call","connector_id":connector,"persist":persist,"tool_params":{"app":"com.example.Fixture","origin":"https://fixture.invalid"},"tool_name":"access_browser_origin"}}})
}
fn snapshot(requests: Vec<Value>) -> Value {
    json!({"cwd":"/fixture","requests":requests,"currentPermissions":{"approvalsReviewer":"user","approvalPolicy":"on-request","sandboxPolicy":{"type":"workspaceWrite"}},"turns":[]})
}

#[test]
fn scopes_and_browser_reviewer_match_source() {
    let state = snapshot(vec![permission(
        "computer-use",
        json!(["session", "always"]),
    )]);
    let found = pending::parse(&state, THREAD, "owner").unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].view["allowsAlways"], true);
    assert_eq!(
        pending::answer(&found[0], &json!({"kind":"allow"}), &state)
            .unwrap()
            .1["_meta"]["persist"],
        "session"
    );
    assert_eq!(
        pending::answer(&found[0], &json!({"kind":"always"}), &state)
            .unwrap()
            .1["_meta"]["persist"],
        "always"
    );
    assert_eq!(
        pending::answer(&found[0], &json!({"kind":"deny"}), &state)
            .unwrap()
            .1,
        json!({"action":"decline","content":null,"_meta":null})
    );
    assert!(pending::parse(
        &snapshot(vec![permission("computer-use", json!(["unknown"]))]),
        THREAD,
        "owner"
    )
    .unwrap()
    .is_empty());
    let mut browser = snapshot(vec![permission("browser-use", Value::Null)]);
    let found = pending::parse(&browser, THREAD, "owner").unwrap();
    assert_eq!(found.len(), 1);
    assert!(
        pending::answer(&found[0], &json!({"kind":"allow"}), &browser)
            .unwrap()
            .1["_meta"]
            .is_null()
    );
    assert!(pending::answer(&found[0], &json!({"kind":"always"}), &browser).is_err());
    browser["currentPermissions"]["approvalsReviewer"] = json!("auto");
    assert!(pending::parse(&browser, THREAD, "owner")
        .unwrap()
        .is_empty());
    browser["currentPermissions"] = json!({"approvalsReviewer":"user","approvalPolicy":"never","sandboxPolicy":{"type":"dangerFullAccess"}});
    assert!(pending::parse(&browser, THREAD, "owner")
        .unwrap()
        .is_empty());
}
#[test]
fn native_cards_say_what_allowing_keeps_as_open_words_it() {
    use agent_hud_core::l10n;
    let conversation = l10n::text("允许此对话", "Allow this conversation");
    let once = l10n::text("批准一次", "Allow once");
    let app = snapshot(vec![permission(
        "computer-use",
        json!(["session", "always"]),
    )]);
    let card = &pending::parse(&app, THREAD, "owner").unwrap()[0].view;
    assert!(
        card["alwaysAllowRule"].is_null(),
        "an app has no rule to show: {card}"
    );
    assert_eq!(card["allowLabel"], json!(conversation));
    assert_eq!(card["badge"], "APP");
    let site = snapshot(vec![permission(
        "browser-use",
        json!(["session", "always"]),
    )]);
    let card = &pending::parse(&site, THREAD, "owner").unwrap()[0].view;
    assert_eq!(
        card["alwaysAllowRule"], "https://fixture.invalid",
        "always allowing adds the site"
    );
    assert_eq!(card["allowLabel"], json!(conversation));
    assert_eq!(card["badge"], "WEB");
    let once_only = snapshot(vec![permission("browser-use", Value::Null)]);
    let card = &pending::parse(&once_only, THREAD, "owner").unwrap()[0].view;
    assert!(card["alwaysAllowRule"].is_null());
    assert_eq!(card["allowLabel"], json!(once));
    // A browser request whose turn carries no settings shows no card; only
    // one no turn is found for falls back on the current permissions.
    let mut unsettled = snapshot(vec![permission("browser-use", Value::Null)]);
    unsettled["turns"] = json!([{"turnId":"turn-fixture"}]);
    assert!(pending::parse(&unsettled, THREAD, "owner")
        .unwrap()
        .is_empty());
    unsettled["turns"] = json!([{"turnId":"turn-fixture","params":{"approvalsReviewer":"user"}}]);
    assert_eq!(
        pending::parse(&unsettled, THREAD, "owner").unwrap().len(),
        1,
        "the turn's own settings count"
    );
}
#[test]
fn canonical_async_question_answer_and_explicit_settlement() {
    let mut state = snapshot(vec![]);
    state["turnHistory"] = json!({"kind":"canonical","history":{"islands":[{"entries":[{"value":"key"}]}],"entitiesByKey":{"key":{"turnId":"turn-fixture","status":"inProgress","items":[{"type":"agentMessage","id":"message","questions":[{"title":"Pick one","options":["First","Second"]}]}]}}}});
    let found = pending::parse(&state, THREAD, "owner").unwrap();
    assert_eq!(found.len(), 1);
    let key = found[0].request_id.as_str().unwrap();
    let (method, response) = pending::answer(
        &found[0],
        &json!({"kind":"answer","answers":{key:{"selected":["First"],"custom":"extra"}}}),
        &state,
    )
    .unwrap();
    assert_eq!(method, "thread-follower-steer-turn");
    assert!(response.as_str().unwrap().contains("First, extra"));
    assert!(pending::answer(
        &found[0],
        &json!({"kind":"answer","answers":{key:{"selected":["Invalid"]}}}),
        &state
    )
    .is_err());
    state["turnHistory"]["history"]["entitiesByKey"]["key"]["items"].as_array_mut().unwrap().push(json!({"type":"steeringUserMessage","status":"accepted","input":[{"type":"text","text":response}]}));
    assert!(pending::parse(&state, THREAD, "owner").unwrap().is_empty());
    state["turns"] = json!([{"turnId":"turn-fixture","status":"completed"}]);
    assert!(pending::parse(&state, THREAD, "owner").is_err()); // overlap cannot prove withdrawal
}
#[test]
fn immer_patch_operations_follow_source() {
    let mut state = json!({"items":[1,2,3],"nested":{"x":1}});
    stream::apply_patches(&json!([{"op":"replace","path":["items","length"],"value":2},{"op":"add","path":["items","-"],"value":4},{"op":"replace","path":["items",3],"value":5},{"op":"remove","path":["items",0]},{"op":"add","path":["nested","y"],"value":2}]),&mut state).unwrap();
    assert_eq!(state, json!({"items":[2,4,5],"nested":{"x":1,"y":2}}));
    stream::apply_patches(&json!([{"op":"nonsense","path":["missing"]},{"op":"replace","path":[],"value":{"ok":true}}]),&mut state).unwrap();
    assert_eq!(state, json!({"ok":true}));
    assert!(stream::apply_patches(
        &json!([{"op":"replace","path":["missing","child"],"value":1}]),
        &mut state
    )
    .is_err());
}

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[test]
fn questions_only_does_not_expose_approval_cards() {
    let requests = Requests::default();
    let mut observer = CodexDesktopObserver::start_at(
        PathBuf::from("unused-fixture-endpoint"),
        requests.clone(),
        Arc::new(|| {}),
    )
    .unwrap();
    observer.set_questions_only(true);
    let state = snapshot(vec![permission("computer-use", json!(["session"]))]);
    observer.shared.reconcile(THREAD, "owner", state);
    assert!(requests.views(crate::hooks::now_ms()).is_empty());
    observer.set_questions_only(false);
    assert_eq!(requests.views(crate::hooks::now_ms()).len(), 1);
    observer.set_questions_only(true);
    assert!(requests.views(crate::hooks::now_ms()).is_empty());
    let old = snapshot(vec![permission("computer-use", json!(["session"]))]);
    let approval = pending::parse(&old, THREAD, "owner").unwrap().remove(0);
    assert!(matches!(
        observer.shared.resolve(&approval, &json!({"kind":"allow"})),
        Err(ReplyFailure::Cancelled)
    ));
    let newer = snapshot(vec![]);
    observer.shared.reconcile(THREAD, "owner", newer.clone());
    observer.set_questions_only(false);
    observer.shared.reconcile_state(THREAD, "owner", old, true);
    assert_eq!(
        observer.shared.state.lock().unwrap().known[THREAD].state,
        newer
    );
    assert!(requests.views(crate::hooks::now_ms()).is_empty());
    observer.stop();
}
