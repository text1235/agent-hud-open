use super::*;
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
};
fn read(socket: &mut UnixStream) -> Value {
    let mut size = [0; 4];
    socket.read_exact(&mut size).unwrap();
    let mut bytes = vec![0; u32::from_le_bytes(size) as usize];
    socket.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn send(socket: &mut UnixStream, value: Value) {
    let data = serde_json::to_vec(&value).unwrap();
    socket
        .write_all(&(data.len() as u32).to_le_bytes())
        .unwrap();
    socket.write_all(&data).unwrap();
}
fn response(socket: &mut UnixStream, request: &Value, result: Value) {
    send(
        socket,
        json!({"type":"response","requestId":request["requestId"],"resultType":"success","method":request["method"],"handledByClientId":"owner","result":result}),
    );
}
fn wait(mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(4);
    while !condition() {
        assert!(Instant::now() < end, "fixture deadline");
        thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn observer_owned_reply_settle_and_disconnect_fixture() {
    for uncertain in [false, true] {
        // No prefix, one-letter names: the IPC socket path must stay short
        // under macOS's long temporary folder.
        let temporary = tempfile::Builder::new().prefix("").tempdir().unwrap();
        let directory = temporary.path().join(if uncertain { "u" } else { "c" });
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(directory.join("ipc")).unwrap();
        let listener = UnixListener::bind(directory.join("ipc/ipc.sock")).unwrap();
        let requests = Requests::default();
        let mut observer =
            CodexDesktopObserver::start(directory.clone(), requests.clone(), Arc::new(|| {}))
                .unwrap();
        let (go, ready) = std::sync::mpsc::channel();
        let (done, ended) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let init = read(&mut stream);
            response(&mut stream, &init, json!({"clientId":"stream-client"}));
            let discovery = read(&mut stream);
            assert_eq!(discovery["method"], "thread-owner-discovery");
            response(&mut stream, &discovery, json!({}));
            assert_eq!(read(&mut stream)["params"]["following"], true);
            let mut state = snapshot(vec![permission(
                "computer-use",
                json!(["session", "always"]),
            )]);
            state["largeToolResult"] = json!("x".repeat(2 * 1024 * 1024));
            send(
                &mut stream,
                json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":THREAD,"change":{"type":"snapshot","revision":1,"conversationState":state}}}),
            );
            let (mut answer, _) = listener.accept().unwrap();
            let init = read(&mut answer);
            response(&mut answer, &init, json!({"clientId":"answer-client"}));
            let discovery = read(&mut answer);
            response(&mut answer, &discovery, json!({}));
            let mutation = read(&mut answer);
            assert_eq!(mutation["method"], pending::ELICITATION_METHOD);
            assert_eq!(mutation["targetClientId"], "owner");
            assert_eq!(mutation["params"]["requestId"], 7);
            assert_eq!(mutation["params"]["response"]["_meta"]["persist"], "always");
            if uncertain {
                drop(answer);
            } else {
                response(
                    &mut answer,
                    &mutation,
                    json!({"ok":true}),
                );
            }
            ready.recv_timeout(Duration::from_secs(4)).unwrap();
            send(
                &mut stream,
                json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":THREAD,"change":{"type":"patches","baseRevision":1,"revision":2,"patches":[{"op":"replace","path":["requests"],"value":[]}]}}}),
            );
            done.send(()).unwrap();
        });
        // A CLI thread in flight is not followed; a Desktop thread is.
        observer.update_sessions(&[json!({"id":THREAD,"source":AGENT.id,"state":"running"})]);
        assert!(observer.shared.wanted().is_empty());
        // Another app-server client's thread opens in Codex, but Desktop does
        // not hold it.
        observer.update_sessions(&[json!({"id":THREAD,"source":AGENT.id,"state":"running","client":"codex_ask","navigationTarget":{"kind":"codex-thread","id":THREAD}})]);
        assert!(observer.shared.wanted().is_empty());
        observer.update_sessions(&[json!({"id":THREAD,"source":AGENT.id,"state":"running","navigationTarget":{"kind":"codex-thread","id":THREAD}})]);
        wait(|| requests.views(crate::hooks::now_ms()).len() == 1);
        assert!(
            observer.shared.state.lock().unwrap().known[THREAD].state["largeToolResult"].is_null()
        );
        let id = requests.views(crate::hooks::now_ms())[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        requests
            .answer(&id, &json!({"kind":"always"}), crate::hooks::now_ms())
            .unwrap();
        wait(|| {
            requests
                .deliveries(crate::hooks::now_ms())
                .iter()
                .any(|row| {
                    row["id"] == id
                        && row["state"] == if uncertain { "uncertain" } else { "confirmed" }
                })
        });
        go.send(()).unwrap();
        ended.recv_timeout(Duration::from_secs(4)).unwrap();
        worker.join().unwrap();
        wait(|| requests.views(crate::hooks::now_ms()).is_empty());
        observer.stop();
        assert!(requests
            .answer(&id, &json!({"kind":"allow"}), crate::hooks::now_ms())
            .is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn native_waiting_is_phase_only_and_clears_on_disconnect_and_stop() {
    let temporary = tempfile::Builder::new().prefix("").tempdir().unwrap();
    let directory = temporary.path().join("w");
    let requests = Requests::default();
    let mut observer =
        CodexDesktopObserver::start(directory, requests.clone(), Arc::new(|| {})).unwrap();
    let mut state = snapshot(vec![]);
    state["threadRuntimeStatus"] = json!({"type":"active","activeFlags":["waitingOnApproval"]});
    observer.shared.reconcile(THREAD, "owner", state.clone());
    assert!(requests
        .native_waiting_sessions()
        .get(AGENT.id)
        .is_some_and(|sessions| sessions.contains(THREAD)));
    assert!(requests.views(crate::hooks::now_ms()).is_empty());
    let revision = requests.revision();
    observer.shared.reconcile(THREAD, "owner", state.clone());
    assert_eq!(requests.revision(), revision);
    state["threadRuntimeStatus"]["activeFlags"] = json!(["waitingOnUserInput"]);
    state.as_object_mut().unwrap().remove("requests");
    observer.shared.reconcile(THREAD, "owner", state.clone()); // waiting is known even if no supported cards can be decoded
    assert!(requests
        .native_waiting_sessions()
        .get(AGENT.id)
        .is_some_and(|sessions| sessions.contains(THREAD)));
    assert!(requests.views(crate::hooks::now_ms()).is_empty());
    observer.shared.forget_connection();
    assert!(requests
        .native_waiting_sessions()
        .get(AGENT.id)
        .is_none_or(|sessions| sessions.is_empty()));
    observer.shared.reconcile(THREAD, "owner", state);
    observer.stop();
    assert!(requests
        .native_waiting_sessions()
        .get(AGENT.id)
        .is_none_or(|sessions| sessions.is_empty()));
    assert!(requests.views(crate::hooks::now_ms()).is_empty());
}
