//! Where a Claude Code session's calls go, as its hook sees it
//! (docs/proposals/no-plan-usage.md §1, §6): the hook runs with the session's
//! environment, `ANTHROPIC_BASE_URL` from settings.json `env` included, so its
//! host and cloud-platform switches are what the session uses. A host on this
//! computer or its network is asked once, when the session starts, what
//! serves it. Only the host is kept, never a path, query or key.
use agent_hud_agents::endpoints::{base_url_host, host_kind, HostKind};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

/// Claude Code's endpoint for the origin event: `host` (None for Anthropic's
/// default), `platform` (bedrock, vertex or foundry when one is switched on)
/// and, for a local host probed at `starting`, `server` and `contextWindow`.
pub(super) fn claude(env: &HashMap<String, String>, starting: bool) -> Value {
    let on = |name: &str| {
        env.get(name)
            .is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "True"))
    };
    let platform = [
        ("CLAUDE_CODE_USE_BEDROCK", "bedrock"),
        ("CLAUDE_CODE_USE_VERTEX", "vertex"),
        ("CLAUDE_CODE_USE_FOUNDRY", "foundry"),
    ]
    .into_iter()
    .find(|(name, _)| on(name))
    .map(|(_, platform)| platform);
    let base = env.get("ANTHROPIC_BASE_URL").map(String::as_str);
    let host = base.and_then(base_url_host);
    let mut endpoint = json!({"host": host, "platform": platform});
    if let (true, Some(host), Some(base)) = (starting, host.as_deref(), base) {
        if host_kind(host) == HostKind::Local && base.trim_start().starts_with("http://") {
            if let Some((server, window)) = probe(host, Duration::from_millis(1_500)) {
                endpoint["server"] = json!(server);
                endpoint["contextWindow"] = json!(window);
            }
        }
    }
    endpoint
}

/// What model server answers at `host` (a local host and port), and the
/// context window it runs with when it says: Ollama, LM Studio, llama.cpp or
/// vLLM. None for anything else (a gateway, nothing listening) or when
/// `budget` runs out.
pub(super) fn probe(host: &str, budget: Duration) -> Option<(&'static str, Option<u64>)> {
    let until = Instant::now() + budget;
    let get = |path: &str| get_json(host, path, until);
    if get("/api/version").is_some_and(|v| v["version"].is_string()) {
        // A loaded model's window, as `ollama ps` shows it.
        let window = get("/api/ps").and_then(|v| v["models"][0]["context_length"].as_u64());
        return Some(("ollama", window));
    }
    if let Some(models) = get("/api/v0/models").filter(|v| v["data"].is_array()) {
        let loaded = models["data"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|model| model["state"] == "loaded");
        let window = loaded.and_then(|model| {
            model["loaded_context_length"]
                .as_u64()
                .or_else(|| model["max_context_length"].as_u64())
        });
        return Some(("lmstudio", window));
    }
    if let Some(props) = get("/props").filter(|v| v["default_generation_settings"].is_object()) {
        return Some((
            "llamacpp",
            props["default_generation_settings"]["n_ctx"].as_u64(),
        ));
    }
    if get("/version").is_some_and(|v| v["version"].is_string()) {
        if let Some(models) = get("/v1/models") {
            if models["data"][0]["owned_by"] == "vllm" {
                return Some(("vllm", models["data"][0]["max_model_len"].as_u64()));
            }
        }
    }
    None
}

/// A local GET of `path` answered with 200 and a JSON body, read before `until`.
fn get_json(host: &str, path: &str, until: Instant) -> Option<Value> {
    let left = |cap: u64| {
        until
            .checked_duration_since(Instant::now())
            .map(|rest| rest.min(Duration::from_millis(cap)))
            .filter(|rest| !rest.is_zero())
    };
    let address = host.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&address, left(300)?).ok()?;
    stream.set_read_timeout(Some(left(800)?)).ok()?;
    stream.set_write_timeout(Some(left(300)?)).ok()?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    stream.take(256 * 1024).read_to_end(&mut response).ok()?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.lines().next()?;
    if status.split_whitespace().nth(1) != Some("200") {
        return None;
    }
    let chunked = head
        .lines()
        .any(|line| line.to_ascii_lowercase().replace(' ', "") == "transfer-encoding:chunked");
    let body = if chunked {
        unchunk(body)
    } else {
        body.to_owned()
    };
    serde_json::from_str(body.trim()).ok()
}

/// A chunked body joined back together.
fn unchunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.trim(), 16) else {
            break;
        };
        if size == 0 || after.len() < size {
            break;
        }
        out.push_str(&after[..size]);
        rest = after[size..].trim_start_matches("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A local server answering each path with its JSON, others with 404.
    fn serve(routes: Vec<(&'static str, &'static str)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buffer = [0u8; 2048];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
                let reply = match routes.iter().find(|(route, _)| *route == path) {
                    Some((_, body)) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    ),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_owned(),
                };
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        host
    }

    #[test]
    fn a_local_model_server_says_what_it_is_and_its_window() {
        let ollama = serve(vec![
            ("/api/version", r#"{"version":"0.12.3"}"#),
            (
                "/api/ps",
                r#"{"models":[{"name":"qwen3-coder:30b","context_length":32768}]}"#,
            ),
        ]);
        assert_eq!(
            probe(&ollama, Duration::from_secs(3)),
            Some(("ollama", Some(32_768)))
        );
        let lmstudio = serve(vec![(
            "/api/v0/models",
            r#"{"object":"list","data":[{"id":"a","state":"not-loaded","max_context_length":4096},{"id":"b","state":"loaded","loaded_context_length":16384,"max_context_length":131072}]}"#,
        )]);
        assert_eq!(
            probe(&lmstudio, Duration::from_secs(3)),
            Some(("lmstudio", Some(16_384)))
        );
        let llamacpp = serve(vec![(
            "/props",
            r#"{"default_generation_settings":{"n_ctx":8192}}"#,
        )]);
        assert_eq!(
            probe(&llamacpp, Duration::from_secs(3)),
            Some(("llamacpp", Some(8_192)))
        );
        let vllm = serve(vec![
            ("/version", r#"{"version":"0.11.0"}"#),
            (
                "/v1/models",
                r#"{"data":[{"id":"m","owned_by":"vllm","max_model_len":65536}]}"#,
            ),
        ]);
        assert_eq!(
            probe(&vllm, Duration::from_secs(3)),
            Some(("vllm", Some(65_536)))
        );
    }

    #[test]
    fn a_gateway_or_nothing_listening_is_no_model_server() {
        let gateway = serve(vec![("/health", r#"{"status":"ok"}"#)]);
        assert_eq!(probe(&gateway, Duration::from_secs(3)), None);
        // A port nobody listens on: bind one, then let it go.
        let free = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .to_string();
        assert_eq!(probe(&free, Duration::from_secs(3)), None);
    }

    #[test]
    fn the_endpoint_keeps_the_host_and_platform_and_probes_only_at_the_start() {
        let server = serve(vec![("/api/version", r#"{"version":"0.12.3"}"#)]);
        let env = |pairs: &[(&str, String)]| {
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.clone()))
                .collect::<HashMap<_, _>>()
        };
        let local = env(&[("ANTHROPIC_BASE_URL", format!("http://{server}/anthropic"))]);
        let started = claude(&local, true);
        assert_eq!(started["host"], server.as_str());
        assert_eq!(started["server"], "ollama");
        assert!(
            claude(&local, false)["server"].is_null(),
            "a prompt does not probe"
        );
        let relay = env(&[(
            "ANTHROPIC_BASE_URL",
            "https://relay.example.com/v1?token=secret".into(),
        )]);
        let relayed = claude(&relay, true);
        assert_eq!(relayed["host"], "relay.example.com");
        assert!(!relayed.to_string().contains("secret"));
        let bedrock = env(&[("CLAUDE_CODE_USE_BEDROCK", "1".into())]);
        assert_eq!(claude(&bedrock, true)["platform"], "bedrock");
        assert_eq!(
            claude(&HashMap::new(), true),
            json!({"host": null, "platform": null})
        );
    }
}
