use std::{os::windows::process::CommandExt, process::Command};

/// Windows has no exec, so the relay still waits on the server here.
pub(super) fn serve(mut command: Command) -> i32 {
    command
        .creation_flags(0x08000000) // CREATE_NO_WINDOW: MCP communicates over inherited stdio.
        .status()
        .map(|status| status.code().unwrap_or(1))
        .unwrap_or(1)
}
