use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "agent-hud-workbench-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("state")).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn state_is_scoped_by_exact_project_path_or_gateway_project_id() {
    let state = scoped_state(
        &json!({
            "projects":[{"id":"selected","path":"z:\\Agent HUD"},{"id":"other","path":"Z:\\Research"}],
            "tasks":[
                {"id":"h","executor":"hermes","project":"selected"},
                {"id":"a","executor":"antigravity","project_root":"Z:/Agent HUD/","workspace":"Z:/Agent HUD/协作办公区/Antigravity/id"},
                {"id":"not-a-child","project_root":"Z:/Agent HUD copy"},
                {"id":"other","project":"other"}
            ]
        }),
        Path::new("Z:/Agent HUD"),
    );
    assert_eq!(state["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(state["projects"].as_array().unwrap().len(), 1);
    assert_eq!(path_key(r"\\?\Z:\Agent HUD"), path_key("z:/agent hud"));
    assert_eq!(
        path_key(r"\\?\UNC\server\share"),
        path_key(r"\\server\share")
    );
}

#[test]
fn flights_describe_real_new_dispatch_delivery_and_failure_only_once() {
    let initial = json!({"tasks":[{"id":"old","executor":"hermes","status":"completed"}]});
    let (known, flights) = transitions(None, &initial);
    assert!(flights.is_empty());
    let running = json!({"tasks":[{"id":"new","executor":"antigravity","status":"running"}]});
    let (known, flights) = transitions(Some(&known), &running);
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].from, "codex");
    assert_eq!(flights[0].to, "antigravity");
    assert!(transitions(Some(&known), &running).1.is_empty());
    let delivered = json!({"tasks":[{"id":"new","executor":"antigravity","status":"completed","review":"pending"}]});
    let (known, flights) = transitions(Some(&known), &delivered);
    assert_eq!(flights.len(), 1);
    assert_eq!(flights[0].from, "antigravity");
    assert_eq!(flights[0].to, "codex");
    assert!(flights[0].arrives);
    assert!(!flights[0].falls);
    assert!(transitions(Some(&known), &delivered).1.is_empty());
    let failed = json!({"tasks":[{"id":"another","executor":"hermes","status":"failed"}]});
    assert!(transitions(Some(&known), &failed).1[0].falls);
}

#[test]
fn antigravity_submission_uses_the_selected_root_fixed_model_and_idempotency_key() {
    let scratch = Scratch::new();
    let selected = Connection {
        root: scratch.0.clone(),
        workbench: scratch.0.clone(),
    };
    let input = json!({"executor":"antigravity","objective":"test","boundary":"only this task","acceptance":"verified output","request_key":"hud-test-1","workspace":"other-project","model":"other-model"});
    let arguments = submission(&selected, &input).unwrap();
    assert_eq!(arguments["workspace"], json!(selected.root));
    assert_eq!(arguments["project_root"], json!(selected.root));
    assert_eq!(arguments["model"], MODEL);
    assert_eq!(arguments["request_key"], "hud-test-1");
    let mut invalid = input;
    invalid["request_key"] = json!("../other");
    assert!(submission(&selected, &invalid).is_err());
}

#[test]
fn http_adapter_reads_local_authorization_and_does_not_forward_it_to_the_ui() {
    let scratch = Scratch::new();
    fs::write(
        scratch.0.join("state/access.token"),
        "test-only-authorization",
    )
    .unwrap();
    let selected = Connection {
        root: scratch.0.clone(),
        workbench: scratch.0.clone(),
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/api", listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 1024];
        let (header_length, content_length) = loop {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&buffer[..read]);
            if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..position]);
                assert!(headers
                    .to_lowercase()
                    .contains("authorization: bearer test-only-authorization"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse().unwrap())
                    })
                    .unwrap();
                break (position + 4, length);
            }
        };
        while bytes.len() < header_length + content_length {
            let read = stream.read(&mut buffer).unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&buffer[..read]);
        }
        let request: Value =
            serde_json::from_slice(&bytes[header_length..header_length + content_length]).unwrap();
        assert_eq!(request, json!({"name":"state","arguments":{}}));
        let response = r#"{"result":{"tasks":[],"projects":[],"executors":[]}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
    });
    let result = call(&client().unwrap(), &endpoint, &selected, "state", json!({})).unwrap();
    assert_eq!(result, json!({"tasks":[],"projects":[],"executors":[]}));
    worker.join().unwrap();
}
