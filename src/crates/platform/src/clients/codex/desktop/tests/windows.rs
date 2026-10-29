use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

async fn read(pipe: &mut NamedPipeServer) -> Value {
    let size = pipe.read_u32_le().await.unwrap();
    assert!(size < 1024 * 1024);
    let mut bytes = vec![0; size as usize];
    pipe.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
async fn send(pipe: &mut NamedPipeServer, message: Value) {
    let bytes = serde_json::to_vec(&message).unwrap();
    pipe.write_u32_le(bytes.len() as u32).await.unwrap();
    pipe.write_all(&bytes).await.unwrap();
}
async fn response(pipe: &mut NamedPipeServer, request: &Value, result: Value) {
    send(pipe, json!({"type":"response","requestId":request["requestId"],"resultType":"success","handledByClientId":"owner","result":result})).await;
}
fn wait(mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < end, "fixture deadline");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn named_pipe_observer_answers_question_and_stops() {
    // An isolated fixture never connects to the installed Codex router.
    let path = PathBuf::from(format!(
        r"\\.\pipe\agenthud-observer-{}",
        desktop_io::request_id().unwrap()
    ));
    let owned = path.clone();
    let (ready, waiting) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut stream = ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&owned).unwrap();
            ready.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(8), async {
                stream.connect().await.unwrap();
                let init = read(&mut stream).await;
                response(&mut stream, &init, json!({"clientId":"stream-client"})).await;
                let discovery = read(&mut stream).await;
                assert_eq!(discovery["method"], "thread-owner-discovery");
                response(&mut stream, &discovery, json!({})).await;
                assert_eq!(read(&mut stream).await["params"]["following"], true);
                let mut state = snapshot(vec![]);
                state["turnHistory"] = json!({"kind":"canonical","history":{"islands":[{"entries":[{"value":"key"}]}],"entitiesByKey":{"key":{"turnId":"turn-fixture","status":"inProgress","items":[{"type":"agentMessage","id":"message","questions":[{"title":"Pick one","options":["First","Second"]}]}]}}}});
                // Prepare the second instance before publishing the card.
                let mut answer = ServerOptions::new().reject_remote_clients(true).create(&owned).unwrap();
                send(&mut stream, json!({"type":"broadcast","method":"thread-stream-state-changed","version":11,"sourceClientId":"owner","params":{"hostId":"local","conversationId":THREAD,"change":{"type":"snapshot","revision":1,"conversationState":state}}})).await;
                answer.connect().await.unwrap();
                let init = read(&mut answer).await;
                response(&mut answer, &init, json!({"clientId":"answer-client"})).await;
                let discovery = read(&mut answer).await;
                response(&mut answer, &discovery, json!({})).await;
                let mutation = read(&mut answer).await;
                assert_eq!(mutation["method"], "thread-follower-steer-turn");
                assert_eq!(mutation["targetClientId"], "owner");
                assert!(mutation.to_string().contains("First"));
                response(&mut answer, &mutation, json!({"method":"thread-follower-steer-turn","result":{"result":{"turnId":"turn-fixture"}}})).await;
                // Wait for the observer to close its stream on stop.
                let mut byte = [0];
                assert_eq!(stream.read(&mut byte).await.unwrap_or(0), 0);
            }).await.unwrap();
        });
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    let requests = Requests::default();
    let mut observer =
        CodexDesktopObserver::start_at(path, requests.clone(), Arc::new(|| {})).unwrap();
    observer.set_questions_only(true);
    observer.update_sessions(&[json!({"source":AGENT.id,"state":"running","navigationTarget":{"kind":"codex-thread","id":THREAD}})]);
    wait(|| requests.views(crate::hooks::now_ms()).len() == 1);
    let view = requests.views(crate::hooks::now_ms()).remove(0);
    let id = view["id"].as_str().unwrap();
    let key = serde_json::to_string(&json!(["request_user_input_async", "message", 0])).unwrap();
    requests
        .answer(
            id,
            &json!({"kind":"answer","answers":{key:{"selected":["First"]}}}),
            crate::hooks::now_ms(),
        )
        .unwrap();
    wait(|| {
        requests
            .deliveries(crate::hooks::now_ms())
            .iter()
            .any(|row| row["id"] == id && row["state"] == "confirmed")
    });
    observer.stop();
    assert!(requests.views(crate::hooks::now_ms()).is_empty());
    worker.join().unwrap();
}
