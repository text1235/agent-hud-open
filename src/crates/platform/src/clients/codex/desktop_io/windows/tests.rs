use super::*;
use crate::clients::codex::desktop_io::{request_id, DesktopIpc};
use serde_json::{json, Value};
use std::{future::Future, sync::mpsc, thread};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

// Every fixture owns a fresh pipe. No test connects to the installed Codex router.
fn serve<F, Fut>(handler: F) -> (PathBuf, thread::JoinHandle<()>)
where
    F: FnOnce(NamedPipeServer) -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
{
    let path = PathBuf::from(format!(
        r"\\.\pipe\agenthud-codex-test-{}",
        request_id().unwrap()
    ));
    let server_path = path.clone();
    let (ready, waiting) = mpsc::channel();
    let worker = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(&server_path)
                .unwrap();
            ready.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                server.connect().await.unwrap();
                handler(server).await;
            })
            .await
            .unwrap();
        });
    });
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    (path, worker)
}

async fn read_message(server: &mut NamedPipeServer) -> Value {
    let mut header = [0; 4];
    server.read_exact(&mut header).await.unwrap();
    let length = u32::from_le_bytes(header) as usize;
    assert!(length > 0 && length < 64 * 1024);
    let mut bytes = vec![0; length];
    server.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn fragmented_message(server: &mut NamedPipeServer, value: Value) {
    let bytes = serde_json::to_vec(&value).unwrap();
    let frame = [(bytes.len() as u32).to_le_bytes().as_slice(), &bytes].concat();
    for part in frame.chunks(3) {
        server.write_all(part).await.unwrap();
    }
}

#[test]
fn same_user_router_keeps_framing_correlation_and_owner_target() {
    let (path, worker) = serve(|mut server| async move {
        let initialize = read_message(&mut server).await;
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["version"], 0);
        assert_eq!(initialize["sourceClientId"], "initializing-client");
        assert_eq!(initialize["params"]["clientType"], "agent-hud");
        fragmented_message(&mut server, json!({"type":"response", "requestId":initialize["requestId"], "resultType":"success", "result":{"clientId":"hud-client"}})).await;
        let discovery = read_message(&mut server).await;
        assert_eq!(discovery["method"], "thread-owner-discovery");
        assert_eq!(discovery["version"], 1);
        assert_eq!(discovery["sourceClientId"], "hud-client");
        assert_eq!(
            discovery["params"],
            json!({"hostId":"local","conversationId":"thread"})
        );
        assert_ne!(initialize["requestId"], discovery["requestId"]);
        fragmented_message(
            &mut server,
            json!({"type":"client-discovery-request", "requestId":"unsolicited"}),
        )
        .await;
        assert_eq!(
            read_message(&mut server).await,
            json!({"type":"client-discovery-response", "requestId":"unsolicited", "response":{"canHandle":false}})
        );
        fragmented_message(
            &mut server,
            json!({"type":"response", "requestId":"unrelated", "resultType":"success"}),
        )
        .await;
        fragmented_message(&mut server, json!({"type":"response", "requestId":discovery["requestId"], "resultType":"success", "handledByClientId":"desktop-owner"})).await;
        let steer = read_message(&mut server).await;
        assert_eq!(steer["method"], "thread-follower-steer-turn");
        assert_eq!(steer["version"], 1);
        assert_eq!(steer["targetClientId"], "desktop-owner");
        assert_eq!(steer["sourceClientId"], "hud-client");
        assert_eq!(steer["params"]["text"], "继续 🪟");
        fragmented_message(&mut server, json!({"type":"response", "requestId":steer["requestId"], "resultType":"success", "handledByClientId":"desktop-owner", "result":{"turnId":"turn"}})).await;
        let _ = server.read(&mut [0]).await;
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(3)).unwrap();
    let initialized = connection
        .request(
            "initialize",
            0,
            "initializing-client",
            None,
            json!({"clientType":"agent-hud"}),
            None,
        )
        .unwrap();
    let client = initialized["result"]["clientId"].as_str().unwrap();
    let discovery = connection
        .request(
            "thread-owner-discovery",
            1,
            client,
            None,
            json!({"hostId":"local","conversationId":"thread"}),
            None,
        )
        .unwrap();
    let owner = discovery["handledByClientId"].as_str().unwrap();
    let mut writes = 0;
    let response = connection
        .request(
            "thread-follower-steer-turn",
            1,
            client,
            Some(owner),
            json!({"text":"继续 🪟"}),
            Some(&mut || writes += 1),
        )
        .unwrap();
    assert_eq!(response["handledByClientId"], owner);
    assert_eq!(response["result"]["turnId"], "turn");
    assert_eq!(writes, 1);
    drop(connection);
    worker.join().unwrap();
}

#[test]
fn truncated_reply_retains_the_mutation_write_marker() {
    let (path, worker) = serve(|mut server| async move {
        read_message(&mut server).await;
        server.write_all(&10u32.to_le_bytes()).await.unwrap();
        server.write_all(b"{").await.unwrap();
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(3)).unwrap();
    let mut wrote = false;
    assert_eq!(
        connection.request(
            "mutation",
            1,
            "client",
            Some("owner"),
            json!({}),
            Some(&mut || wrote = true)
        ),
        Err(IpcError::Unavailable)
    );
    assert!(wrote);
    worker.join().unwrap();
}

#[test]
fn partial_reply_expires_at_the_operation_deadline() {
    let (path, worker) = serve(|mut server| async move {
        read_message(&mut server).await;
        server.write_all(&10u32.to_le_bytes()).await.unwrap();
        server.write_all(b"{").await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(3)).unwrap();
    connection.renew(Duration::from_millis(100));
    let mut wrote = false;
    assert_eq!(
        connection.request(
            "mutation",
            1,
            "client",
            Some("owner"),
            json!({}),
            Some(&mut || wrote = true)
        ),
        Err(IpcError::TimedOut)
    );
    assert!(wrote);
    worker.join().unwrap();
}

#[test]
fn oversized_frame_is_rejected_before_reading_its_body() {
    let (path, worker) = serve(|mut server| async move {
        read_message(&mut server).await;
        server.write_all(&1025u32.to_le_bytes()).await.unwrap();
        let _ = server.read(&mut [0]).await;
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(3)).unwrap();
    connection.post(json!({"ready":true})).unwrap();
    assert_eq!(connection.receive_with_limit(1024), Err(IpcError::Protocol));
    drop(connection);
    worker.join().unwrap();
}

#[test]
fn cancellation_interrupts_a_partial_reply_and_keeps_the_write_marker() {
    let (partial, waiting) = mpsc::channel();
    let (path, server) = serve(|mut pipe| async move {
        read_message(&mut pipe).await;
        pipe.write_all(&10u32.to_le_bytes()).await.unwrap();
        pipe.write_all(b"{").await.unwrap();
        partial.send(()).unwrap();
        let _ = pipe.read(&mut [0]).await;
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(30)).unwrap();
    let shutdown = connection.shutdown_handle().unwrap();
    let reader = thread::spawn(move || {
        let mut wrote = false;
        let result = connection.request(
            "mutation",
            1,
            "client",
            Some("owner"),
            json!({}),
            Some(&mut || wrote = true),
        );
        (result, wrote)
    });
    waiting.recv_timeout(Duration::from_secs(3)).unwrap();
    let stopped = Instant::now();
    shutdown.shutdown().unwrap();
    let (result, wrote) = reader.join().unwrap();
    assert_eq!(result, Err(IpcError::Unavailable));
    assert!(
        wrote,
        "a cancelled reply cannot turn an accepted write into a retry"
    );
    assert!(stopped.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}

#[test]
fn cancellation_wakes_readiness_and_remains_set_after_renew() {
    let (path, server) = serve(|mut pipe| async move {
        let _ = pipe.read(&mut [0]).await;
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(30)).unwrap();
    let shutdown = connection.shutdown_handle().unwrap();
    let (started, waiting) = mpsc::channel();
    let reader = thread::spawn(move || {
        started.send(()).unwrap();
        let result = connection.wait_for_message(Duration::from_secs(30));
        connection.renew(Duration::from_secs(30));
        let after_renew = connection.receive();
        (result, after_renew)
    });
    waiting.recv_timeout(Duration::from_secs(3)).unwrap();
    let stopped = Instant::now();
    shutdown.shutdown().unwrap();
    let (result, after_renew) = reader.join().unwrap();
    assert_eq!(result, Err(IpcError::Unavailable));
    assert_eq!(after_renew, Err(IpcError::Unavailable));
    assert!(stopped.elapsed() < Duration::from_secs(1));
    server.join().unwrap();
}

#[test]
fn cancellation_before_post_does_not_report_a_write() {
    let (path, server) = serve(|mut pipe| async move {
        assert_eq!(pipe.read(&mut [0]).await.unwrap(), 0);
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(3)).unwrap();
    connection.shutdown_handle().unwrap().shutdown().unwrap();
    let mut wrote = false;
    assert_eq!(
        connection.post_with_write(json!({"mutation":true}), Some(&mut || wrote = true)),
        Err(IpcError::Unavailable)
    );
    assert!(!wrote);
    drop(connection);
    server.join().unwrap();
}

#[test]
fn readiness_after_an_exact_frame_read_does_not_block_the_answer_loop() {
    let (release, released) = tokio::sync::oneshot::channel();
    let (path, server) = serve(|mut pipe| async move {
        fragmented_message(&mut pipe, json!({"ready":true})).await;
        let _ = released.await;
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(3)).unwrap();
    assert_eq!(connection.receive().unwrap(), json!({"ready":true}));
    assert_eq!(connection.wait_for_message(Duration::from_millis(50)), Ok(false));
    release.send(()).unwrap();
    drop(connection);
    server.join().unwrap();
}

#[test]
fn cancellation_interrupts_continuous_writes_without_a_reader() {
    let (release, released) = tokio::sync::oneshot::channel();
    let (path, server) = serve(|pipe| async move {
        let _ = released.await;
        drop(pipe);
    });
    let mut connection = DesktopIpc::connect(&path, Duration::from_secs(30)).unwrap();
    let shutdown = connection.shutdown_handle().unwrap();
    let (wrote, waiting) = mpsc::channel();
    let writer = thread::spawn(move || {
        let payload = json!({"payload":"x".repeat(1024 * 1024)});
        let mut first = true;
        loop {
            let result = connection.post_with_write(payload.clone(), Some(&mut || {
                if first { wrote.send(()).unwrap(); first = false; }
            }));
            if result.is_err() { break result; }
        }
    });
    waiting.recv_timeout(Duration::from_secs(3)).unwrap();
    let stopped = Instant::now();
    shutdown.shutdown().unwrap();
    assert_eq!(writer.join().unwrap(), Err(IpcError::Unavailable));
    assert!(stopped.elapsed() < Duration::from_secs(1));
    release.send(()).unwrap();
    server.join().unwrap();
}
