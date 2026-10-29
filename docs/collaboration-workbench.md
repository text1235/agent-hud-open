# Local collaboration workbench

This fork adds a **Collaboration workbench / 协作工作台** Settings page. It uses
the existing Hermes/Antigravity workbench at `http://127.0.0.1:18500/api`.
It does not run another Gateway, replace the Antigravity desktop bridge, or
change Hermes's Weixin connection.

## Connect

Start the existing workbench normally. In Agent HUD Settings, open the
Collaboration workbench page, choose an existing **business project root** and
the existing workbench installation directory, then connect. The token is
read from `<workbench directory>/state/access.token` by the native adapter;
it is never sent to the webview or saved in Agent HUD's preferences. HTTP
requests bypass proxies and refuse redirects.

The selected directory paths are saved in Agent HUD's own app-data
`workbench.json`. The connection resumes on next launch. Disconnect removes
this preference and stops observation; it does not stop either executor or
the existing workbench service.

The page and its task-details endpoint show only the exact selected project.
Hermes tasks are matched through their registered Gateway project ID;
Antigravity tasks through their `project_root`. A similarly named folder or
another project's office directory does not match.

## Dispatch and delivery

The manual form requires an objective, modification boundary and acceptance
criteria. Clicking Send performs a real submission. Antigravity always uses
`gemini-4-argon-high` and the existing desktop service, which verifies its
native model and project binding. The workbench creates its project-local
office directory. Hermes remains registered against the business project
root, and its task boundary names an office under
`协作办公区/大肥鱼/hud-<request key>`.

The native adapter sends one request and does not retry a submission. The
form retains its unique request key after an error and locks that submission.
If the response is unknown, inspect the existing task list before starting
another task. Task details only resume observation when the existing
workbench supports it; the HUD does not resend an executor prompt.

Manual HUD submissions do **not** bind a Hermes delivery queue to the current
Codex chat. They do not automatically wake Codex, create a review record or
pass independent acceptance. For the full Codex dispatch/delivery/review
loop, submit through the existing Gateway tools in the originating Codex
chat. The HUD displays `completed` as **已交付**, separately from the recorded
`accepted` / **验收通过** review.

After connecting, the adapter observes workbench task state every three
seconds. Newly seen queued/running tasks animate from Codex to their
executor; completion animates back, and failure/cancellation drops the
flight. Existing tasks on first connection establish a baseline and do not
replay historical animations. Stable status does not repeat an animation.
These flights use Agent HUD's existing `HostContext.fly` interface and its
Hermes/Antigravity artwork. Connection errors retain the last observed tasks
and mark the connection unavailable.

The observer has one owned thread, a condition-variable wakeup and bounded
HTTP timeouts. Normal app shutdown stops and joins it. It does not create
executor processes or terminate user applications.

## Validation

`src/apps/desktop/src-tauri/src/workbench/tests.rs` covers exact project
filtering, initial-history suppression, one-time dispatch/delivery/failure
flights, fixed model/project submission parameters, request-key validation,
and a mock HTTP server that checks authorization and request framing without
accessing a live executor. Run with `cargo test -p agent-hud-next workbench`.

Live acceptance additionally needs the existing local service and the built
desktop app: connect the intended project, inspect its task list, submit a
bounded test task, observe the real task state and animation, then inspect
the actual deliverable before recording any independent acceptance.
