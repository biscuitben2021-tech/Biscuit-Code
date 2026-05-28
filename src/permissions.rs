use crate::tools::ToolCall;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

// ── Enums ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Manual,
    Assisted,
    Auto,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Assisted => "Assisted",
            Self::Auto => "Auto",
        }
    }
    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Manual => "Ask before every action",
            Self::Assisted => "Auto safe actions, ask before edits",
            Self::Auto => "Work without asking inside approved workspace",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[allow(dead_code)]
pub enum Action {
    ReadFile,
    WriteFile,
    CreateFile,
    DeleteFile,
    MoveFile,
    RunReadonlyCmd,
    RunProjectCmd,
    RunInstallCmd,
    RunAdminCmd,
    BrowserRead,
    BrowserFormSubmit,
    ComputerView,
    ComputerControl,
    SendEmail,
    SendMessage,
    UploadFile,
    DownloadFile,
    UseApi,
    UseMcp,
    AccessOutsideWorkspace,
    AccessSecret,
    MakePayment,
    ChangeSystemSettings,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadFile => "Read file",
            Self::WriteFile => "Edit file",
            Self::CreateFile => "Create file",
            Self::DeleteFile => "Delete file",
            Self::MoveFile => "Move/rename file",
            Self::RunReadonlyCmd => "Run read-only command",
            Self::RunProjectCmd => "Run project command",
            Self::RunInstallCmd => "Install package",
            Self::RunAdminCmd => "Run admin/sudo command",
            Self::BrowserRead => "Read webpage",
            Self::BrowserFormSubmit => "Submit web form",
            Self::ComputerView => "View screen",
            Self::ComputerControl => "Control computer",
            Self::SendEmail => "Send email",
            Self::SendMessage => "Send message",
            Self::UploadFile => "Upload file",
            Self::DownloadFile => "Download file",
            Self::UseApi => "Use API",
            Self::UseMcp => "Use MCP tool",
            Self::AccessOutsideWorkspace => "Access outside workspace",
            Self::AccessSecret => "Access secret/password",
            Self::MakePayment => "Make payment",
            Self::ChangeSystemSettings => "Change system settings",
        }
    }

    fn risk(self) -> &'static str {
        match self {
            Self::ReadFile | Self::RunReadonlyCmd | Self::BrowserRead | Self::ComputerView => "Low",
            Self::WriteFile
            | Self::CreateFile
            | Self::MoveFile
            | Self::RunProjectCmd
            | Self::RunInstallCmd
            | Self::UseMcp
            | Self::UseApi
            | Self::BrowserFormSubmit
            | Self::ComputerControl
            | Self::UploadFile
            | Self::DownloadFile => "Medium",
            Self::DeleteFile | Self::SendEmail | Self::SendMessage => "High",
            Self::RunAdminCmd
            | Self::AccessOutsideWorkspace
            | Self::AccessSecret
            | Self::MakePayment
            | Self::ChangeSystemSettings => "Critical",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Allow,
    Ask,
    Block,
}

// ── Permission Matrix ───────────────────────────────────────────────────

pub fn check(mode: Mode, action: Action) -> Decision {
    use Action::*;
    use Decision::*;
    // Always blocked regardless of mode
    match action {
        RunAdminCmd
        | AccessOutsideWorkspace
        | AccessSecret
        | MakePayment
        | ChangeSystemSettings => return Block,
        _ => {}
    }
    match mode {
        Mode::Manual => Ask,
        Mode::Assisted => match action {
            ReadFile | RunReadonlyCmd | BrowserRead | ComputerView => Allow,
            _ => Ask,
        },
        Mode::Auto => match action {
            SendEmail | SendMessage => Block,
            _ => Allow,
        },
    }
}

// ── Classify tool call → Action ─────────────────────────────────────────

pub fn classify(call: &ToolCall) -> Action {
    let s = |key: &str| {
        call.args
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    match call.tool.to_lowercase().as_str() {
        "read" | "glob" | "grep" => Action::ReadFile,
        "observe" | "observation" => Action::ReadFile,
        "goal" | "plan" => Action::ReadFile, // planning is safe
        "websearch" => Action::BrowserRead,
        "webfetch" => Action::BrowserRead,
        "askuserquestion" => Action::ReadFile, // asking user is always fine
        "write" => {
            if bool_val(&call.args, "overwrite") {
                Action::WriteFile
            } else {
                Action::CreateFile
            }
        }
        "edit" => Action::WriteFile,
        "bash" => classify_bash(s("command")),
        "monitor" => {
            let action = s("action");
            if action == "read" || (action.is_empty() && call.args.get("command").is_none()) {
                Action::RunReadonlyCmd
            } else {
                Action::RunProjectCmd
            }
        }
        "computeruse" | "computer_use" | "computer-use" | "computer use" => {
            let action = s("action").to_lowercase();
            match action.as_str() {
                "screenshot" | "state" | "open" => Action::ComputerView,
                _ => Action::ComputerControl,
            }
        }
        "mcp" => {
            let action = s("action").to_lowercase();
            match action.as_str() {
                "list" | "tools" => Action::ReadFile,
                _ => Action::UseMcp,
            }
        }
        "slashcommand" => Action::ReadFile,
        _ => Action::UseApi,
    }
}

fn classify_bash(cmd: &str) -> Action {
    let cmd = cmd.trim();
    let first = cmd.split_whitespace().next().unwrap_or("");
    // Admin / dangerous
    if first == "sudo" || cmd.starts_with("rm -rf /") {
        return Action::RunAdminCmd;
    }
    // Install commands
    for prefix in [
        "npm install",
        "npm i ",
        "yarn add",
        "pip install",
        "pip3 install",
        "cargo install",
        "cargo add",
        "brew install",
        "apt install",
        "apt-get install",
    ] {
        if cmd.starts_with(prefix) {
            return Action::RunInstallCmd;
        }
    }
    // Read-only commands
    let ro = [
        "ls", "pwd", "cat", "head", "tail", "wc", "file", "find", "which", "echo", "date",
        "whoami", "env", "printenv", "tree", "du", "df",
    ];
    if ro.contains(&first) {
        return Action::RunReadonlyCmd;
    }
    // Git read-only
    if first == "git" {
        let rest = cmd.strip_prefix("git").unwrap_or("").trim();
        let sub = rest.split_whitespace().next().unwrap_or("");
        let ro_git = [
            "status",
            "diff",
            "log",
            "show",
            "branch",
            "remote",
            "tag",
            "stash list",
            "rev-parse",
        ];
        if ro_git.contains(&sub) || rest.starts_with("stash list") {
            return Action::RunReadonlyCmd;
        }
    }
    // grep family
    if matches!(first, "grep" | "rg" | "ag" | "ack") {
        return Action::RunReadonlyCmd;
    }
    Action::RunProjectCmd
}

fn bool_val(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

// ── Log Entry ───────────────────────────────────────────────────────────

struct LogEntry {
    timestamp: String,
    mode: Mode,
    action: Action,
    tool: String,
    target: String,
    result: Decision,
    user_decision: Option<String>,
}

impl LogEntry {
    fn summary(&self) -> String {
        let ud = self
            .user_decision
            .as_deref()
            .map(|d| format!(" (user: {d})"))
            .unwrap_or_default();
        format!(
            "{} [{:?}] {:?} → {:?}{} | {} | {}",
            self.timestamp, self.mode, self.action, self.result, ud, self.tool, self.target
        )
    }
}

// ── Permission Guard ────────────────────────────────────────────────────

pub struct PermissionGuard {
    pub mode: Mode,
    log: Vec<LogEntry>,
    pub(crate) task_allowed: HashSet<Action>,
    config_path: PathBuf,
    pub stop_requested: AtomicBool,
}

impl PermissionGuard {
    pub fn open(workspace: &Path) -> Self {
        let config_path = workspace.join(".biscuits/permissions.json");
        let mode = fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| serde_json::from_str::<ModeFile>(&s).ok())
            .map(|f| f.mode)
            .unwrap_or(Mode::Assisted);
        Self {
            mode,
            log: Vec::new(),
            task_allowed: HashSet::new(),
            config_path,
            stop_requested: AtomicBool::new(false),
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.task_allowed.clear();
        self.save();
        if mode == Mode::Auto {
            eprintln!(
                "\n⚠  Auto Mode lets the agent work without asking inside approved workspaces."
            );
            eprintln!("   Dangerous system-level actions remain blocked.");
            eprintln!("   Type '/permissions assisted' to switch back anytime.\n");
        }
    }

    fn save(&self) {
        let file = ModeFile { mode: self.mode };
        if let Some(parent) = self.config_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(
            &self.config_path,
            serde_json::to_string_pretty(&file).unwrap_or_default(),
        );
    }

    /// Gate a tool call. Returns Ok(()) to proceed, Err(msg) to skip.
    pub fn gate(&mut self, call: &ToolCall) -> Result<(), String> {
        if self.stop_requested.load(Ordering::Relaxed) {
            return Err("agent stopped by user".into());
        }
        let action = classify(call);
        let mut decision = check(self.mode, action);

        // "Always allow for this task" override
        if decision == Decision::Ask && self.task_allowed.contains(&action) {
            decision = Decision::Allow;
        }

        let target = call_target(call);
        let timestamp = now_iso();

        match decision {
            Decision::Allow => {
                self.log(timestamp, action, call, &target, Decision::Allow, None);
                Ok(())
            }
            Decision::Ask => {
                let user = self.prompt_approval(action, call, &target);
                match user.as_str() {
                    "allow" => {
                        self.log(
                            timestamp,
                            action,
                            call,
                            &target,
                            Decision::Ask,
                            Some("allow once"),
                        );
                        Ok(())
                    }
                    "always" => {
                        self.task_allowed.insert(action);
                        self.log(
                            timestamp,
                            action,
                            call,
                            &target,
                            Decision::Ask,
                            Some("always allow"),
                        );
                        Ok(())
                    }
                    "switch" => {
                        self.prompt_switch_mode();
                        // Re-check with new mode
                        let new_decision = check(self.mode, action);
                        if new_decision == Decision::Allow {
                            self.log(
                                timestamp,
                                action,
                                call,
                                &target,
                                Decision::Allow,
                                Some("mode switched"),
                            );
                            Ok(())
                        } else if new_decision == Decision::Block {
                            self.log(
                                timestamp,
                                action,
                                call,
                                &target,
                                Decision::Block,
                                Some("blocked after switch"),
                            );
                            Err(format!(
                                "action blocked: {} is not allowed in {} mode",
                                action.label(),
                                self.mode.label()
                            ))
                        } else {
                            // Still Ask — treat as deny to avoid infinite loop
                            self.log(
                                timestamp,
                                action,
                                call,
                                &target,
                                Decision::Ask,
                                Some("denied after switch"),
                            );
                            Err(format!("action denied: {}", action.label()))
                        }
                    }
                    _ => {
                        self.log(
                            timestamp,
                            action,
                            call,
                            &target,
                            Decision::Ask,
                            Some("denied"),
                        );
                        Err(format!("action denied by user: {}", action.label()))
                    }
                }
            }
            Decision::Block => {
                self.log(timestamp, action, call, &target, Decision::Block, None);
                Err(format!(
                    "action blocked: {} is never allowed in {} mode",
                    action.label(),
                    self.mode.label()
                ))
            }
        }
    }

    fn log(
        &mut self,
        timestamp: String,
        action: Action,
        call: &ToolCall,
        target: &str,
        result: Decision,
        user_decision: Option<&str>,
    ) {
        self.log.push(LogEntry {
            timestamp,
            mode: self.mode,
            action,
            tool: call.tool.clone(),
            target: target.to_string(),
            result,
            user_decision: user_decision.map(String::from),
        });
    }

    fn prompt_approval(&self, action: Action, call: &ToolCall, target: &str) -> String {
        let risk = action.risk();
        eprintln!();
        eprintln!("┌─ Permission Required ─────────────────────────────┐");
        eprintln!("│ Action:  {:<41}│", action.label());
        eprintln!("│ Tool:    {:<41}│", call.tool);
        eprintln!("│ Target:  {:<41}│", truncate_str(target, 41));
        eprintln!("│ Risk:    {:<41}│", risk);
        eprintln!("│                                                   │");
        eprintln!("│  [1] Allow once                                   │");
        eprintln!("│  [2] Always allow this action type for this task  │");
        eprintln!("│  [3] Deny                                         │");
        eprintln!("│  [4] Switch permission mode                       │");
        eprintln!("└───────────────────────────────────────────────────┘");
        eprint!("choice> ");
        let _ = io::stderr().flush();

        let mut input = String::new();
        let _ = io::stdin().read_line(&mut input);
        match input.trim() {
            "1" => "allow".into(),
            "2" => "always".into(),
            "3" => "deny".into(),
            "4" => "switch".into(),
            other => {
                if other.eq_ignore_ascii_case("allow") {
                    "allow".into()
                } else {
                    "deny".into()
                }
            }
        }
    }

    fn prompt_switch_mode(&mut self) {
        eprintln!("\nSwitch permission mode:");
        eprintln!("  [1] Manual  — Ask before every action");
        eprintln!("  [2] Assisted — Auto safe actions, ask before edits");
        eprintln!("  [3] Auto    — Work without asking inside workspace");
        eprint!("choice> ");
        let _ = io::stderr().flush();

        let mut input = String::new();
        let _ = io::stdin().read_line(&mut input);
        let new_mode = match input.trim() {
            "1" | "manual" => Mode::Manual,
            "2" | "assisted" => Mode::Assisted,
            "3" | "auto" => Mode::Auto,
            _ => {
                eprintln!("invalid choice, keeping current mode");
                return;
            }
        };
        self.set_mode(new_mode);
        eprintln!("switched to {} mode", new_mode.label());
    }

    // ── Slash command handler ───────────────────────────────────────────

    pub fn command_output(&mut self, input: &str) -> Option<String> {
        let input = input.trim();
        if input == "/permissions" {
            return Some(self.status_summary());
        }
        let rest = input.strip_prefix("/permissions ")?.trim();
        match rest {
            "manual" => {
                self.set_mode(Mode::Manual);
                Some(format!("permission mode set to {}", Mode::Manual.label()))
            }
            "assisted" => {
                self.set_mode(Mode::Assisted);
                Some(format!("permission mode set to {}", Mode::Assisted.label()))
            }
            "auto" => {
                self.set_mode(Mode::Auto);
                Some(format!("permission mode set to {}", Mode::Auto.label()))
            }
            "log" => Some(self.log_summary()),
            "stop" => {
                self.stop_requested.store(true, Ordering::Relaxed);
                Some("stop requested — agent will halt after current action".into())
            }
            "info" => Some(self.info()),
            _ => Some(format!(
                "unknown: {rest}\nusage: /permissions [manual|assisted|auto|log|stop|info]"
            )),
        }
    }

    fn status_summary(&self) -> String {
        let recent: Vec<String> = self
            .log
            .iter()
            .rev()
            .take(5)
            .map(LogEntry::summary)
            .collect();
        let recent_str = if recent.is_empty() {
            "  (none yet)".into()
        } else {
            recent.into_iter().rev().collect::<Vec<_>>().join("\n  ")
        };
        format!(
            "permission mode: {} — {}\nrecent actions:\n  {}",
            self.mode.label(),
            self.mode.subtitle(),
            recent_str
        )
    }

    fn log_summary(&self) -> String {
        if self.log.is_empty() {
            return "no actions logged yet".into();
        }
        let entries: Vec<String> = self
            .log
            .iter()
            .rev()
            .take(20)
            .map(LogEntry::summary)
            .collect();
        format!(
            "permission log (last {}):\n{}",
            entries.len(),
            entries.into_iter().rev().collect::<Vec<_>>().join("\n")
        )
    }

    fn info(&self) -> String {
        format!(
            r#"Agent Permission Modes:

  Manual   — Ask before every action
    Allow: thinking, planning, chat only
    Ask:   all tool actions (read, edit, run, browse, etc.)
    Block: dangerous system actions

  Assisted — Auto safe actions, ask before edits  [default]
    Allow: read files, search, web browse, read-only commands, screenshots
    Ask:   edits, creates, deletes, project commands, installs, MCP, computer control
    Block: dangerous system actions

  Auto     — Work without asking inside approved workspace
    Allow: all normal workspace actions (read, write, run, browse, MCP)
    Ask:   (none)
    Block: admin/sudo commands, outside workspace access, secrets, payments,
           system settings, sending emails/messages

Current mode: {} — {}
Stored at:    {}"#,
            self.mode.label(),
            self.mode.subtitle(),
            self.config_path.display()
        )
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn call_target(call: &ToolCall) -> String {
    let s = |key: &str| {
        call.args
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match call.tool.to_lowercase().as_str() {
        "read" | "write" | "edit" => s("path"),
        "bash" => s("command"),
        "glob" | "grep" => s("pattern"),
        "websearch" => s("query"),
        "webfetch" => s("url"),
        "monitor" => format!("{} {}", s("action"), s("command")),
        "computeruse" | "computer_use" | "computer-use" | "computer use" => s("action"),
        "mcp" => format!("{}/{}", s("server"), s("name")),
        _ => call
            .args
            .as_object()
            .and_then(|m| m.keys().next())
            .map(|k| s(k))
            .unwrap_or_default(),
    }
}

fn now_iso() -> String {
    // Minimal ISO-ish timestamp without chrono dependency
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = d.as_secs();
    let s = total_secs % 60;
    let m = (total_secs / 60) % 60;
    let h = (total_secs / 3600) % 24;
    let total_days = total_secs / 86400;
    // Days since 1970-01-01 → year/month/day (simplified civil calendar)
    let mut y: i64 = 1970;
    let mut remaining = total_days as i64;
    loop {
        let days_in_year: i64 = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = is_leap(y);
    let month_days: [i64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1;
    for &md in &month_days {
        if remaining < md {
            break;
        }
        remaining -= md;
        mo += 1;
    }
    let day = remaining + 1;
    format!("{y:04}-{mo:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}…",
            s.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}

#[derive(Serialize, Deserialize)]
struct ModeFile {
    mode: Mode,
}
