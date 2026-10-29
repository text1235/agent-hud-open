//! Windows does not expose the Unix daemon's terminal-origin socket.
/// The Codex daemon's control socket is a Unix socket; none is read on Windows.
pub(crate) fn daemon_tui_origin_for(
    _thread_id: &str,
    _home: &std::path::Path,
    _env: &std::collections::HashMap<String, String>,
) -> Option<String> {
    None
}
