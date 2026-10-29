//! The existing local collaboration workbench, scoped to one selected project.
//! This adapter never runs an executor or marks its output as accepted.
use agent_hud_agents::{antigravity, codex, hermes};
use agent_hud_host::extension::{Flight, HostContext, ProductExtension};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
use tauri::Manager;

const ENDPOINT: &str = "http://127.0.0.1:18500/api";
const MODEL: &str = "gemini-4-argon-high";

#[derive(Clone, PartialEq, Eq)]
struct Connection {
    root: PathBuf,
    workbench: PathBuf,
}

struct State {
    active: bool,
    connection: Option<Connection>,
    preferences: Option<PathBuf>,
    snapshot: Value,
    known: Option<HashMap<String, String>>,
}

pub struct Workbench {
    shared: Arc<(Mutex<State>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Default for Workbench {
    fn default() -> Self {
        Self {
            shared: Arc::new((
                Mutex::new(State {
                    active: false,
                    connection: None,
                    preferences: None,
                    snapshot: json!({"connected": false, "tasks": []}),
                    known: None,
                }),
                Condvar::new(),
            )),
            worker: Mutex::new(None),
        }
    }
}

impl ProductExtension for Workbench {
    fn setup_native(&self, app: &tauri::AppHandle) -> Result<(), String> {
        let file = app
            .path()
            .app_data_dir()
            .map_err(|e| e.to_string())?
            .join("workbench.json");
        let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Ok(bytes) = fs::read(&file) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                match connection(&value) {
                    Ok(connection) => state.connection = Some(connection),
                    Err(error) => state.snapshot["error"] = json!(error),
                }
            }
        }
        state.preferences = Some(file);
        Ok(())
    }

    fn start(&self, host: HostContext) -> Result<(), String> {
        let mut worker = self.worker.lock().unwrap_or_else(|p| p.into_inner());
        if worker.is_some() {
            return Ok(());
        }
        self.shared
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active = true;
        let shared = self.shared.clone();
        *worker = Some(
            thread::Builder::new()
                .name("Collaboration workbench".into())
                .spawn(move || {
                    let client = client();
                    loop {
                        let selected = {
                            let state = shared.0.lock().unwrap_or_else(|p| p.into_inner());
                            if !state.active {
                                break;
                            }
                            state.connection.clone()
                        };
                        if let Some(selected) = selected {
                            let result = client.as_ref().map_err(Clone::clone).and_then(|client| {
                                call(client, ENDPOINT, &selected, "state", json!({}))
                            });
                            let mut state = shared.0.lock().unwrap_or_else(|p| p.into_inner());
                            if state.connection.as_ref() == Some(&selected) && state.active {
                                match result {
                                    Ok(value) => {
                                        let value = scoped_state(&value, &selected.root);
                                        let (known, flights) =
                                            transitions(state.known.as_ref(), &value);
                                        state.known = Some(known);
                                        state.snapshot = value;
                                        drop(state);
                                        for flight in flights {
                                            (host.fly)(flight);
                                        }
                                    }
                                    Err(error) => {
                                        state.snapshot["connected"] = json!(false);
                                        state.snapshot["error"] = json!(error);
                                    }
                                }
                            }
                        }
                        let state = shared.0.lock().unwrap_or_else(|p| p.into_inner());
                        if !state.active {
                            break;
                        }
                        let _ = shared.1.wait_timeout(state, Duration::from_secs(3));
                    }
                })
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "workbench_status" => {
                let state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
                let mut value = state.snapshot.clone();
                if let Some(connection) = &state.connection {
                    value["projectRoot"] = json!(connection.root);
                    value["workbenchRoot"] = json!(connection.workbench);
                    value["automaticDispatch"] = crate::auto_dispatch::status(&connection.root)?;
                }
                Ok(value)
            }
            "workbench_connect" => {
                let selected = connection(&params)?;
                // Verify authorization and availability before saving this connection.
                let value = scoped_state(
                    &call(&client()?, ENDPOINT, &selected, "state", json!({}))?,
                    &selected.root,
                );
                let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(file) = &state.preferences {
                    if let Some(parent) = file.parent() {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    fs::write(file, serde_json::to_vec(&json!({"projectRoot":selected.root,"workbenchRoot":selected.workbench})).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                }
                state.known = Some(transitions(None, &value).0);
                state.connection = Some(selected);
                state.snapshot = value.clone();
                self.shared.1.notify_all();
                Ok(value)
            }
            "workbench_disconnect" => {
                let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(file) = &state.preferences {
                    match fs::remove_file(file) {
                        Ok(()) => (),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                        Err(error) => return Err(error.to_string()),
                    }
                }
                state.connection = None;
                state.known = None;
                state.snapshot = json!({"connected":false,"tasks":[]});
                self.shared.1.notify_all();
                Ok(state.snapshot.clone())
            }
            "workbench_auto_dispatch" => {
                let root = self
                    .shared
                    .0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .connection
                    .as_ref()
                    .map(|connection| connection.root.clone())
                    .ok_or("请先连接项目工作台")?;
                let enabled = params["enabled"]
                    .as_bool()
                    .ok_or("自动派工设置必须为开关值")?;
                crate::auto_dispatch::enable(&root, enabled)
            }
            "workbench_task" | "workbench_submit" => {
                let selected = self
                    .shared
                    .0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .connection
                    .clone()
                    .ok_or("请先连接项目工作台")?;
                let client = client()?;
                let executor = text(&params, "executor")?;
                if executor != hermes::AGENT.id && executor != antigravity::AGENT.id {
                    return Err("执行端无效".into());
                }
                let (name, arguments) = if method == "workbench_task" {
                    let id = text(&params, "task_id")?;
                    let state = scoped_state(
                        &call(&client, ENDPOINT, &selected, "state", json!({}))?,
                        &selected.root,
                    );
                    if !state["tasks"].as_array().is_some_and(|tasks| {
                        tasks
                            .iter()
                            .any(|task| task["id"] == id && task["executor"] == executor)
                    }) {
                        return Err("任务不属于当前项目".into());
                    }
                    ("task", json!({"executor":executor,"task_id":id}))
                } else {
                    ("submit", submission(&selected, &params)?)
                };
                let result = call(&client, ENDPOINT, &selected, name, arguments);
                self.shared.1.notify_all();
                if name == "submit" {
                    result.map_err(|error| {
                        format!(
                            "派工未确认：{error}。请先刷新任务；保留相同提交标识，勿另建重复任务。"
                        )
                    })
                } else {
                    result
                }
            }
            _ => Err(format!("Unknown workbench method: {method}")),
        }
    }

    fn stop(&self) {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active = false;
        self.shared.1.notify_all();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Workbench {
    fn drop(&mut self) {
        self.stop();
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key]
        .as_str()
        .filter(|v| !v.trim().is_empty() && v.len() <= 20000)
        .ok_or_else(|| format!("请填写 {key}"))
}

fn directory(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.is_absolute() || !path.is_dir() {
        return Err("请选择已存在的项目或工作台绝对目录".into());
    }
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        Ok(PathBuf::from(format!(r"\\{unc}")))
    } else if let Some(normal) = text.strip_prefix(r"\\?\") {
        Ok(PathBuf::from(normal))
    } else {
        Ok(path)
    }
}

fn connection(value: &Value) -> Result<Connection, String> {
    Ok(Connection {
        root: directory(text(value, "projectRoot")?)?,
        workbench: directory(text(value, "workbenchRoot")?)?,
    })
}

fn client() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|_| "无法建立本机工作台连接".into())
}

fn call(
    client: &Client,
    endpoint: &str,
    selected: &Connection,
    name: &str,
    arguments: Value,
) -> Result<Value, String> {
    let token = fs::read_to_string(selected.workbench.join("state/access.token"))
        .map_err(|_| "无法读取所选工作台的本机授权文件".to_string())?;
    let response = client
        .post(endpoint)
        .bearer_auth(token.trim())
        .json(&json!({"name":name,"arguments":arguments}))
        .send()
        .map_err(|_| "本机工作台无响应".to_string())?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err("工作台本机授权失效，请核对所选工作台目录".into());
    }
    if !response.status().is_success() {
        return Err(format!(
            "工作台拒绝请求（HTTP {}），请在原工作台核对任务状态",
            response.status().as_u16()
        ));
    }
    let value: Value = response
        .json()
        .map_err(|_| "工作台返回了无效响应".to_string())?;
    if let Some(error) = value["error"].as_str() {
        return Err(error.into());
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "工作台响应缺少结果".into())
}

fn path_key(path: &str) -> String {
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).into()
    }
    .replace('\\', "/");
    let path = path.trim_end_matches('/');
    if path.as_bytes().get(1) == Some(&b':') || path.starts_with("//") {
        path.to_lowercase()
    } else {
        path.into()
    }
}

fn scoped_state(value: &Value, root: &Path) -> Value {
    let key = path_key(&root.to_string_lossy());
    let projects: Vec<Value> = value["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["path"].as_str().is_some_and(|p| path_key(p) == key))
        .cloned()
        .collect();
    let ids: HashSet<&str> = projects.iter().filter_map(|p| p["id"].as_str()).collect();
    let tasks: Vec<Value> = value["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            t["project_root"]
                .as_str()
                .or_else(|| t["workspace"].as_str())
                .is_some_and(|p| path_key(p) == key)
                || t["project"].as_str().is_some_and(|id| ids.contains(id))
        })
        .cloned()
        .collect();
    json!({"connected":true,"projectRoot":root,"projects":projects,"tasks":tasks,"executors":value["executors"],"dispatchExecutors":[{"id":hermes::AGENT.id,"name":"大肥鱼"},{"id":antigravity::AGENT.id,"name":"Antigravity"}]})
}

fn submission(selected: &Connection, params: &Value) -> Result<Value, String> {
    let executor = text(params, "executor")?;
    let request_key = text(params, "request_key")?;
    if !request_key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || request_key.len() > 100
    {
        return Err("提交标识只能包含字母、数字和短横线，最多100字符".into());
    }
    let objective = text(params, "objective")?;
    let boundary = text(params, "boundary")?;
    let acceptance = text(params, "acceptance")?;
    let boundary = if executor == hermes::AGENT.id {
        let office = selected
            .root
            .join("协作办公区/大肥鱼")
            .join(format!("hud-{request_key}"));
        fs::create_dir_all(&office).map_err(|e| e.to_string())?;
        format!("{boundary}\n所属项目根目录：{}\n本次办公区：{}\n新增成果只写入本次办公区。禁止递归转派Codex；保留原有程序和微信连接。", selected.root.display(), office.display())
    } else {
        boundary.into()
    };
    Ok(
        json!({"executor":executor,"workspace":selected.root,"project_root":selected.root,"objective":objective,"boundary":boundary,"acceptance":acceptance,"request_key":request_key,"model":MODEL}),
    )
}

fn transitions(
    previous: Option<&HashMap<String, String>>,
    state: &Value,
) -> (HashMap<String, String>, Vec<Flight>) {
    let mut known = HashMap::new();
    let mut flights = Vec::new();
    for task in state["tasks"].as_array().into_iter().flatten() {
        let (Some(id), Some(executor), Some(status)) = (
            task["id"].as_str(),
            task["executor"].as_str(),
            task["status"].as_str(),
        ) else {
            continue;
        };
        let key = format!("workbench:{executor}:{id}");
        if let Some(previous) = previous {
            let old = previous.get(&key).map(String::as_str);
            if old != Some(status) {
                let completed = status == "completed";
                let failed = matches!(status, "failed" | "cancelled");
                let dispatched = old.is_none() && matches!(status, "queued" | "running");
                if completed || failed || dispatched {
                    flights.push(Flight {
                        lane: key.clone(),
                        from: if dispatched {
                            codex::AGENT.id
                        } else {
                            executor
                        }
                        .into(),
                        to: if dispatched {
                            executor
                        } else {
                            codex::AGENT.id
                        }
                        .into(),
                        from_left: dispatched,
                        arrives: !failed,
                        falls: failed,
                    });
                }
            }
        }
        known.insert(key, status.into());
    }
    (known, flights)
}

#[cfg(test)]
mod tests;
