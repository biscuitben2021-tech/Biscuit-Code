use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

/// Native launcher for the **Biscuit Browser** plugin — the self-contained
/// Electron app under `biscuit-browser/`. `/browser use` installs its
/// dependencies on first run (if needed) and starts it in the background.
///
/// Biscuit Browser does not need to be connected over MCP for this command to
/// drive it; the CLI launches it directly. (Once running, the browser also
/// exposes its own MCP server so the agent can read pages / act through tools.)
pub struct BrowserRuntime {
    dir: PathBuf,
    child: Option<Child>,
}

impl BrowserRuntime {
    pub fn new(workspace: &Path) -> Self {
        Self {
            dir: workspace.join("biscuit-browser"),
            child: None,
        }
    }

    /// Handle `/browser …`. Returns `Ok(None)` if this isn't a `/browser` command.
    pub fn command_output(&mut self, input: &str) -> Result<Option<String>> {
        let Some(rest) = input.strip_prefix("/browser") else {
            return Ok(None);
        };
        // Don't match longer command names (e.g. a hypothetical "/browsers").
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return Ok(None);
        }

        let output = match rest.trim() {
            "" | "status" => self.status(),
            "use" | "open" | "launch" | "start" => self.launch()?,
            "stop" | "quit" | "close" => self.stop(),
            "help" => Self::help(),
            other => format!("unknown /browser command: {other}\n\n{}", Self::help()),
        };
        Ok(Some(output))
    }

    fn present(&self) -> bool {
        self.dir.join("package.json").is_file()
    }

    fn installed(&self) -> bool {
        self.dir.join("node_modules").is_dir()
    }

    /// Whether the process launched from this session is still alive (reaps it
    /// if it has exited).
    fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(_)) => {
                    self.child = None;
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.child = None;
                    false
                }
            },
            None => false,
        }
    }

    fn status(&mut self) -> String {
        if !self.present() {
            return format!(
                "Biscuit Browser is not present.\nExpected at: {}\n(Run this from the Biscuits repo root.)",
                self.dir.display()
            );
        }
        let running = if self.is_running() {
            "running (this session)"
        } else {
            "not running"
        };
        let deps = if self.installed() {
            "dependencies installed"
        } else {
            "dependencies NOT installed (they'll install on `/browser use`)"
        };
        format!(
            "Biscuit Browser: {running}; {deps}.\nDirectory: {}\nStart it with `/browser use`.",
            self.dir.display()
        )
    }

    fn launch(&mut self) -> Result<String> {
        if !self.present() {
            bail!(
                "Biscuit Browser not found at {} — run this from the Biscuits repo root.",
                self.dir.display()
            );
        }
        if self.is_running() {
            return Ok("Biscuit Browser is already running (launched from this session).".into());
        }

        let needs_install = !self.installed();
        // sh -lc / cmd /C both understand `&&`, so install (if needed) then run.
        let cmd = if needs_install {
            "npm install && npm run dev"
        } else {
            "npm run dev"
        };

        let child = crate::shell::command(cmd)
            .current_dir(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to launch Biscuit Browser ({cmd}) in {}",
                    self.dir.display()
                )
            })?;
        self.child = Some(child);

        let installing = if needs_install {
            " Installing dependencies first — the window may take a minute to appear."
        } else {
            ""
        };
        Ok(format!(
            "Launching Biscuit Browser in the background.{installing}\n\
             Once the window is up, its agent API (MCP) is at http://127.0.0.1:8765/mcp.\n\
             `/browser status` to check, `/browser stop` to close it."
        ))
    }

    fn stop(&mut self) -> String {
        match self.child.take() {
            Some(mut child) => {
                let _ = child.kill();
                let _ = child.wait();
                "Stopped the Biscuit Browser launcher. Close the window if it's still open.".into()
            }
            None => "Biscuit Browser was not launched from this session (nothing to stop).".into(),
        }
    }

    fn help() -> String {
        "Biscuit Browser — native launcher\n\
         usage: /browser [use|status|stop]\n  \
         /browser use     install (if needed) and launch the browser\n  \
         /browser status  show whether it is present / installed / running\n  \
         /browser stop    stop the browser launched from this session"
            .into()
    }
}
