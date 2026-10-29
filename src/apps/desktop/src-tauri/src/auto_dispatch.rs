//! Project-local instructions for Codex's own dispatch and acceptance decisions.
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    ops::Range,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const START: &str = "<!-- agent-hud:auto-dispatch:start -->";
const END: &str = "<!-- agent-hud:auto-dispatch:end -->";
const BLOCK: &str = "\n\n<!-- agent-hud:auto-dispatch:start -->\n## Agent HUD automatic dispatch\n\nRead and follow `.agent-hud/automatic-dispatch.md` for this project's automatic delegation and independent acceptance workflow.\n<!-- agent-hud:auto-dispatch:end -->\n";
const TEMPLATE: &str = include_str!("../../../../../docs/integrations/automatic-dispatch.md");

fn root(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || !path.is_dir() {
        return Err("请选择已存在的项目绝对目录".into());
    }
    fs::canonicalize(path).map_err(|error| error.to_string())
}

fn local(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("项目自动派工文件或目录是链接，请先在项目内使用普通文件夹".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn paths(root: &Path) -> Result<(PathBuf, PathBuf), String> {
    let agents = root.join("AGENTS.md");
    let own = root.join(".agent-hud");
    local(&agents)?;
    local(&own)?;
    local(&own.join("automatic-dispatch.md"))?;
    Ok((agents, own))
}

fn read(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn block(text: &str) -> Result<Option<Range<usize>>, String> {
    let starts = text.matches(START).count();
    let ends = text.matches(END).count();
    if starts == 0 && ends == 0 {
        return Ok(None);
    }
    if starts == 1 && ends == 1 {
        if let Some(start) = text.find(BLOCK) {
            return Ok(Some(start..start + BLOCK.len()));
        }
    }
    Err("AGENTS.md 的 Agent HUD 管理区已修改或标记不完整；已保留原文件，请先核对".into())
}

fn instructions(root: &Path) -> String {
    let text = root.to_string_lossy();
    let display = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
    };
    TEMPLATE.replace("{{PROJECT_ROOT}}", &display)
}

fn unique() -> String {
    format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        std::process::id()
    )
}

fn backup(own: &Path, original: &str) -> Result<(), String> {
    let folder = own.join("backup");
    local(&folder)?;
    fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join(format!("AGENTS-{}.md", unique())))
        .map_err(|error| error.to_string())?;
    file.write_all(original.as_bytes())
        .map_err(|error| error.to_string())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    let temporary = path.with_extension(format!("agent-hud-{}.tmp", unique()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| error.to_string())
}

/// Inspect the exact managed block and its canonical instruction file.
pub fn status(project_root: &Path) -> Result<Value, String> {
    let root = root(project_root)?;
    let (agents, own) = paths(&root)?;
    let original = read(&agents)?.unwrap_or_default();
    let present = block(&original)?.is_some();
    let file = own.join("automatic-dispatch.md");
    let current = read(&file)?;
    Ok(json!({
        "enabled": present && current.as_deref() == Some(instructions(&root).as_str()),
        "managedBlockPresent": present,
        "projectRoot": root,
        "agentsPath": agents,
        "instructionsPath": file,
    }))
}

/// Enable or remove only Agent HUD's exact block; preserve all other instructions.
pub fn enable(project_root: &Path, enabled: bool) -> Result<Value, String> {
    let root = root(project_root)?;
    let (agents, own) = paths(&root)?;
    let existing = read(&agents)?;
    let original = existing.as_deref().unwrap_or_default();
    let managed = block(original)?;
    let mut next = original.to_string();
    if enabled {
        fs::create_dir_all(&own).map_err(|error| error.to_string())?;
        let canonical = instructions(&root);
        let file = own.join("automatic-dispatch.md");
        if read(&file)?.as_deref() != Some(&canonical) {
            write(&file, &canonical)?;
        }
        if managed.is_none() {
            next.push_str(BLOCK);
        }
    } else if let Some(range) = managed {
        next.replace_range(range, "");
    }
    if next != original {
        if let Some(existing) = existing {
            backup(&own, &existing)?;
        }
        write(&agents, &next)?;
    }
    status(&root)
}

#[cfg(test)]
mod tests;
