//! How an Antigravity process is told apart: its command-line grammar, its
//! language server's `--app_data_dir` flag and the app bundle that hosts it,
//! for the hooks' origin.
use crate::naming::surfaces::Category;
use crate::process::{Caller, Origin};
use serde_json::json;
use std::path::Path;

/// Antigravity's command-line program.
pub const PROGRAM: &str = "agy";
/// The bundle of the Antigravity desktop app.
const DESKTOP_BUNDLE: &str = "com.google.antigravity";

/// Whether a path names an Antigravity CLI or language-server executable.
fn is_antigravity_binary(path: &Path) -> bool {
    crate::process::native_program(path, PROGRAM)
        || crate::process::native_program(path, "antigravity")
        || crate::process::native_program(path, "agentapi")
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("language_server"))
}

/// The `--app_data_dir` value passed to an ancestor `language_server` process.
fn app_data_dir<'a>(caller: &'a Caller) -> Option<&'a str> {
    for ancestor in caller.ancestors {
        let mut iter = ancestor.arguments.iter();
        while let Some(arg) = iter.next() {
            if arg == "--app_data_dir" {
                if let Some(val) = iter.next() {
                    return Some(val.as_str());
                }
            } else if let Some(val) = arg.strip_prefix("--app_data_dir=") {
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// Whether CLI arguments start an interactive terminal session rather than a
/// one-shot prompt or `agentapi` command.
fn interactive(arguments: &[String]) -> bool {
    for argument in arguments.iter().skip(1) {
        if argument == "--" {
            break;
        }
        let option = argument.split('=').next().unwrap_or("");
        if matches!(
            option,
            "-p" | "--print" | "--prompt" | "--headless" | "agentapi"
        ) {
            return false;
        }
    }
    true
}

/// Where an Antigravity session runs: the desktop app (`antigravity`), the IDE
/// (`antigravity-ide`), or the terminal CLI (`agy` / `antigravity-cli`).
pub fn origin(caller: &Caller, session: &str) -> Origin {
    let verified = caller.ancestors.iter().any(|process| {
        is_antigravity_binary(&process.executable)
            || process
                .resolved
                .as_deref()
                .is_some_and(|p| is_antigravity_binary(p) || (caller.installed)(p))
            || process.bundle.as_deref() == Some(DESKTOP_BUNDLE)
    });
    if !verified {
        return Origin::unsupported();
    }

    let data_dir = app_data_dir(caller);
    let cli_ancestor = caller.ancestors.iter().find(|process| {
        crate::process::native_program(&process.executable, PROGRAM)
            || crate::process::native_program(&process.executable, "antigravity")
            || process
                .resolved
                .as_deref()
                .is_some_and(|p| (caller.installed)(p))
    });

    if data_dir == Some("antigravity-cli")
        || (cli_ancestor.is_some()
            && !matches!(data_dir, Some("antigravity" | "antigravity-ide"))
            && !caller.under_bundle(DESKTOP_BUNDLE))
    {
        let is_interactive = cli_ancestor.is_none_or(|proc| interactive(&proc.arguments));
        return Origin {
            host: "cli",
            target: serde_json::Value::Null,
            interactive: is_interactive,
            ask_channel: false,
            category: Some(if is_interactive {
                Category::Cli
            } else {
                Category::Sdk
            }),
            app: None,
        };
    }

    let is_ide = data_dir == Some("antigravity-ide")
        || caller.ancestors.iter().any(|process| {
            process
                .executable
                .to_str()
                .is_some_and(|s| s.contains("Antigravity IDE.app"))
        });

    if is_ide || data_dir == Some("antigravity") || caller.under_bundle(DESKTOP_BUNDLE) {
        let target = if session.is_empty() {
            serde_json::Value::Null
        } else {
            json!({"kind": "antigravity-conversation", "id": session})
        };
        return Origin {
            host: "desktop",
            target,
            interactive: false,
            ask_channel: false,
            category: Some(if is_ide {
                Category::Ide
            } else {
                Category::Desktop
            }),
            app: Some("antigravity"),
        };
    }

    Origin::unsupported()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::Ancestor;
    use std::collections::HashMap;

    fn run(
        executable: &str,
        arguments: &[&str],
        bundles: &[&str],
        installed_path: Option<&str>,
    ) -> Origin {
        let mut ancestors = vec![Ancestor {
            executable: executable.into(),
            resolved: Some(executable.into()),
            arguments: arguments.iter().map(|s| (*s).to_owned()).collect(),
            bundle: None,
        }];
        for bundle in bundles {
            ancestors.push(Ancestor {
                bundle: Some((*bundle).into()),
                ..Ancestor::default()
            });
        }
        let env = HashMap::new();
        let installed =
            |path: &Path| installed_path.is_some_and(|expected| path == Path::new(expected));
        let caller = Caller {
            ancestors: &ancestors,
            env: &env,
            installed: &installed,
        };
        origin(&caller, "conv-1")
    }

    #[test]
    fn distinguishes_desktop_ide_and_cli_origins() {
        let desktop = run(
            "/Applications/Antigravity.app/Contents/Resources/bin/language_server",
            &["language_server", "--app_data_dir", "antigravity"],
            &[DESKTOP_BUNDLE],
            None,
        );
        assert_eq!(desktop.host, "desktop");
        assert_eq!(
            desktop.target,
            json!({"kind": "antigravity-conversation", "id": "conv-1"})
        );
        assert_eq!(
            (desktop.category, desktop.app),
            (Some(Category::Desktop), Some("antigravity"))
        );

        let ide = run(
            "/Applications/Antigravity IDE.app/Contents/Resources/bin/language_server",
            &["language_server", "--app_data_dir=antigravity-ide"],
            &[],
            None,
        );
        assert_eq!(ide.host, "desktop");
        assert_eq!(
            (ide.category, ide.app),
            (Some(Category::Ide), Some("antigravity"))
        );

        let cli = run("/Users/me/.local/bin/agy", &["agy"], &[], None);
        assert_eq!((cli.host, cli.interactive), ("cli", true));
        assert_eq!(cli.target, serde_json::Value::Null);
        assert_eq!((cli.category, cli.app), (Some(Category::Cli), None));

        let cli_ls = run(
            "/Users/me/.local/bin/language_server",
            &["language_server", "--app_data_dir", "antigravity-cli"],
            &[],
            None,
        );
        assert_eq!((cli_ls.host, cli_ls.interactive), ("cli", true));
        assert_eq!((cli_ls.category, cli_ls.app), (Some(Category::Cli), None));

        let headless = run(
            "/Users/me/.local/bin/agy",
            &["agy", "-p", "fix bug"],
            &[],
            None,
        );
        assert_eq!((headless.host, headless.interactive), ("cli", false));
        assert_eq!(
            (headless.category, headless.app),
            (Some(Category::Sdk), None)
        );

        let unknown = run("/usr/bin/python3", &["python3"], &[], None);
        assert_eq!(unknown, Origin::unsupported());
    }
}
