//! User-defined lifecycle hooks. The agent runs shell commands the user configures
//! in `.biscuits/hooks.json` around tool calls and at turn end. A `pre_tool` hook
//! that exits non-zero blocks the tool call (and its output is fed back to the
//! model), which lets a user enforce policy the model can't bypass.
//!
//! Example `.biscuits/hooks.json`:
//! ```json
//! {
//!   "pre_tool":  ["./scripts/guard.sh"],
//!   "post_tool": ["echo \"$BISCUITS_TOOL ran\" >> .biscuits/tool.log"],
//!   "stop":      ["say done"]
//! }
//! ```
//! Hooks receive context via environment variables: `BISCUITS_HOOK_EVENT`,
//! `BISCUITS_TOOL`, `BISCUITS_TOOL_ARGS` (JSON), and for `post_tool`
//! `BISCUITS_TOOL_RESULT`.

use crate::shell;
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

const HOOK_TIMEOUT_SECS: u64 = 30;

#[derive(Default, Deserialize)]
struct HookConfig {
    #[serde(default)]
    pre_tool: Vec<String>,
    #[serde(default)]
    post_tool: Vec<String>,
    #[serde(default)]
    stop: Vec<String>,
}

pub struct Hooks {
    workspace: PathBuf,
    config: HookConfig,
}

#[derive(Default)]
pub struct HookOutcome {
    /// A pre-tool hook vetoed the tool call.
    pub blocked: bool,
    /// Lines to surface to the model (hook stdout/stderr, or the block reason).
    pub messages: Vec<String>,
}

impl Hooks {
    pub fn open(workspace: &Path) -> Self {
        let path = workspace.join(".biscuits/hooks.json");
        let config = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            workspace: workspace.to_path_buf(),
            config,
        }
    }

    /// Whether any hooks are configured (skip all work when not).
    pub fn active(&self) -> bool {
        !self.config.pre_tool.is_empty()
            || !self.config.post_tool.is_empty()
            || !self.config.stop.is_empty()
    }

    /// Run pre-tool hooks. Any hook exiting non-zero blocks the tool call.
    pub fn pre_tool(&self, tool: &str, args: &Value) -> HookOutcome {
        let mut outcome = HookOutcome::default();
        for cmd in &self.config.pre_tool {
            let envs = [
                ("BISCUITS_HOOK_EVENT", "pre_tool".to_string()),
                ("BISCUITS_TOOL", tool.to_string()),
                ("BISCUITS_TOOL_ARGS", args.to_string()),
            ];
            let (code, out) = self.run_one(cmd, &envs);
            let out = out.trim().to_string();
            if code != 0 {
                outcome.blocked = true;
                let detail = if out.is_empty() {
                    String::new()
                } else {
                    format!(": {out}")
                };
                outcome
                    .messages
                    .push(format!("blocked by pre-tool hook (exit {code}){detail}"));
                break;
            } else if !out.is_empty() {
                outcome.messages.push(format!("pre-tool hook: {out}"));
            }
        }
        outcome
    }

    pub fn post_tool(&self, tool: &str, result: &str) {
        for cmd in &self.config.post_tool {
            let envs = [
                ("BISCUITS_HOOK_EVENT", "post_tool".to_string()),
                ("BISCUITS_TOOL", tool.to_string()),
                (
                    "BISCUITS_TOOL_RESULT",
                    result.chars().take(8000).collect::<String>(),
                ),
            ];
            let _ = self.run_one(cmd, &envs);
        }
    }

    pub fn stop(&self) {
        for cmd in &self.config.stop {
            let envs = [("BISCUITS_HOOK_EVENT", "stop".to_string())];
            let _ = self.run_one(cmd, &envs);
        }
    }

    /// Run a single hook command, returning (exit code, combined stdout+stderr).
    /// Drains pipes on threads and kills the process group on timeout (so a hook
    /// can neither deadlock nor orphan grandchildren).
    fn run_one(&self, cmd: &str, envs: &[(&str, String)]) -> (i32, String) {
        let mut command = shell::command(cmd);
        command
            .current_dir(&self.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        shell::spawn_in_own_group(&mut command);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => return (-1, format!("hook failed to start: {err}")),
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let out_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut s) = stdout {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });
        let err_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_end(&mut buf);
            }
            buf
        });

        let start = Instant::now();
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(_) => break,
            }
            if start.elapsed() > Duration::from_secs(HOOK_TIMEOUT_SECS) {
                timed_out = true;
                shell::kill_tree(&mut child);
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }

        let status = child.wait().ok();
        let code = if timed_out {
            -1
        } else {
            status.and_then(|s| s.code()).unwrap_or(-1)
        };
        let mut out = String::from_utf8_lossy(&out_handle.join().unwrap_or_default()).into_owned();
        let err = String::from_utf8_lossy(&err_handle.join().unwrap_or_default()).into_owned();
        if !err.trim().is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(err.trim());
        }
        if timed_out {
            out.push_str("\n[hook timed out]");
        }
        (code, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hooks_with(config: HookConfig) -> Hooks {
        Hooks {
            workspace: std::env::temp_dir(),
            config,
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn pre_tool_hook_nonzero_exit_blocks_the_call() {
        let hooks = hooks_with(HookConfig {
            pre_tool: vec!["echo nope >&2; exit 3".to_string()],
            ..Default::default()
        });
        let outcome = hooks.pre_tool("bash", &json!({"command": "rm -rf /"}));
        assert!(outcome.blocked);
        assert!(outcome.messages.iter().any(|m| m.contains("exit 3")));
        assert!(outcome.messages.iter().any(|m| m.contains("nope")));
    }

    #[cfg(not(windows))]
    #[test]
    fn pre_tool_hook_zero_exit_allows_and_surfaces_output() {
        let hooks = hooks_with(HookConfig {
            pre_tool: vec!["echo looks-fine".to_string()],
            ..Default::default()
        });
        let outcome = hooks.pre_tool("read", &json!({"path": "x"}));
        assert!(!outcome.blocked);
        assert!(outcome.messages.iter().any(|m| m.contains("looks-fine")));
    }

    #[test]
    fn no_hooks_is_inactive() {
        assert!(!hooks_with(HookConfig::default()).active());
    }
}
