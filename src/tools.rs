use crate::{
    computer_use::ComputerUseRuntime, goals::GoalStore, mcp::McpManager,
    observations::ObservationSystem, skills::SkillStore,
};
use anyhow::{bail, Context, Result};
use glob::{glob, Pattern};
use regex::Regex;
use reqwest::{header::USER_AGENT, Client};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

pub struct ToolRuntime {
    workspace: PathBuf,
    goals: GoalStore,
    observations: ObservationSystem,
    computer_use: ComputerUseRuntime,
    mcp: McpManager,
    skills: SkillStore,
    monitors: HashMap<u64, Monitor>,
    next_monitor: u64,
}

struct Monitor {
    command: String,
    child: Child,
    rx: Receiver<String>,
    buffer: Vec<String>,
}

#[derive(Clone)]
pub struct ToolCall {
    pub tool: String,
    pub args: Value,
}

impl ToolCall {
    pub fn activity_title(&self) -> String {
        let lower = self.tool.to_lowercase();
        match lower.as_str() {
            "bash" => self
                .args
                .get("command")
                .and_then(Value::as_str)
                .map(|command| format!("Bash: {}", one_line(command, 96)))
                .unwrap_or_else(|| "Bash".into()),
            "read" | "write" | "edit" => self
                .args
                .get("path")
                .and_then(Value::as_str)
                .map(|path| format!("{}: {}", self.tool, one_line(path, 96)))
                .unwrap_or_else(|| self.tool.clone()),
            "grep" => self
                .args
                .get("pattern")
                .and_then(Value::as_str)
                .map(|pattern| format!("Grep: {}", one_line(pattern, 96)))
                .unwrap_or_else(|| self.tool.clone()),
            "glob" => self
                .args
                .get("pattern")
                .and_then(Value::as_str)
                .map(|pattern| format!("Glob: {}", one_line(pattern, 96)))
                .unwrap_or_else(|| self.tool.clone()),
            "monitor" => self
                .args
                .get("command")
                .and_then(Value::as_str)
                .map(|command| format!("Monitor: {}", one_line(command, 96)))
                .unwrap_or_else(|| "Monitor".into()),
            "mcp" => self
                .args
                .get("action")
                .and_then(Value::as_str)
                .map(|action| format!("Mcp: {action}"))
                .unwrap_or_else(|| "Mcp".into()),
            "computeruse" | "computer_use" | "computer-use" | "computer use" => self
                .args
                .get("action")
                .or_else(|| self.args.get("type"))
                .and_then(Value::as_str)
                .map(|action| format!("ComputerUse: {action}"))
                .unwrap_or_else(|| "ComputerUse".into()),
            "slashcommand" => self
                .args
                .get("command")
                .and_then(Value::as_str)
                .map(|command| format!("SlashCommand: {}", one_line(command, 96)))
                .unwrap_or_else(|| "SlashCommand".into()),
            _ => self.tool.clone(),
        }
    }

    pub fn activity_detail(&self) -> String {
        let args =
            serde_json::to_string_pretty(&self.args).unwrap_or_else(|_| self.args.to_string());
        let mut out = format!("tool: {}\nargs:\n{}", self.tool, args);
        if self.tool.eq_ignore_ascii_case("bash") {
            if let Some(command) = self.args.get("command").and_then(Value::as_str) {
                out.push_str(&format!("\n\nbash command:\n{command}"));
            }
        }
        out
    }

    pub fn needs_user_input(&self) -> bool {
        self.tool.eq_ignore_ascii_case("askuserquestion")
    }

    pub fn needs_permission_input(&self, perms: &crate::permissions::PermissionGuard) -> bool {
        let action = crate::permissions::classify(self);
        let decision = crate::permissions::check(perms.mode, action);
        decision == crate::permissions::Decision::Ask && !perms.task_allowed.contains(&action)
    }
}

impl ToolRuntime {
    pub fn new(workspace: PathBuf) -> Result<Self> {
        let workspace = workspace.canonicalize().unwrap_or(workspace);
        let goals = GoalStore::open(&workspace)?;
        let observations = ObservationSystem::new(workspace.clone());
        let computer_use = ComputerUseRuntime::new(workspace.clone());
        let mcp = McpManager::open(&workspace)?;
        let skills = SkillStore::open(&workspace)?;
        Ok(Self {
            workspace,
            goals,
            observations,
            computer_use,
            mcp,
            skills,
            monitors: HashMap::new(),
            next_monitor: 1,
        })
    }

    /// Returns the `<selected_skills>` system block for this user message, or
    /// an empty string when no enabled skill is relevant. Selection depends on
    /// the message, so this is computed per turn rather than in `system_prompt`.
    pub fn skills_context(&self, user_message: &str) -> String {
        self.skills.selected_context(user_message)
    }

    pub fn system_prompt(&self) -> String {
        format!(
            r#"<tool_instructions>
You have workspace tools for: {workspace}
When a tool is needed, return only one or more blocks shaped exactly:
<tool_call>{{"tool":"Read","args":{{"path":"src/main.rs"}}}}</tool_call>
After tool results are provided, continue reasoning from the result.
Available tools:
- Read: {{"path":"relative/file","max_bytes":20000}}
- Write: {{"path":"relative/file","content":"...","overwrite":false}}
- Edit: {{"path":"relative/file","old":"exact text","new":"replacement","replace_all":false}}
- Bash: {{"command":"cargo test","timeout_secs":60}}
- Monitor: {{"action":"start","command":"npm run dev"}} or {{"action":"read","id":1}} or {{"action":"stop","id":1}}
- Glob: {{"pattern":"**/*.rs"}}
- Grep: {{"pattern":"regex","path":".","glob":"**/*.rs","max_matches":100}}
- WebSearch: {{"query":"search terms","limit":5}}
- WebFetch: {{"url":"https://example.com","max_chars":12000}}
- AskUserQuestion: {{"question":"...","choices":["A","B"]}}
- ComputerUse: inspect and control the local GUI using screenshots and screen pixel coordinates, e.g. {{"action":"open","target":"http://localhost:3000","wait_ms":1500}}, {{"action":"screenshot"}}, {{"action":"click","x":640,"y":420,"button":"left"}}, {{"action":"move","x":640,"y":420}}, {{"action":"type","text":"hello"}}, {{"action":"key","key":"cmd+l"}}. Use it to open apps/websites, screenshot to verify, then click/type through the UI.
- SlashCommand: run any local slash command the user can type, e.g. {{"command":"/help"}}, {{"command":"/computer-use screenshot"}}, {{"command":"/mcp list"}}, {{"command":"/privacy incognito"}}. Do not use /exit or /quit.
- Mcp: connect and use MCP servers, e.g. {{"action":"connect","name":"filesystem","command":"npx -y @modelcontextprotocol/server-filesystem ."}}, {{"action":"tools","server":"filesystem"}}, {{"action":"call","server":"filesystem","name":"read_file","arguments":{{"path":"README.md"}}}}
- Observe: {{"target":"workspace|changes|file|terminal|monitor|web|screen","path":"relative/file","id":1,"url":"https://example.com"}}
- Goal: manage the requirement todo list, e.g. {{"action":"set","title":"...","requirements":["..."]}}, {{"action":"update_requirement","id":"R1","status":"done","evidence":"..."}}, or {{"action":"mark_done","requirements_met":true,"verified":true,"no_errors":true,"checks":["cargo test"],"evidence":["..."]}}
- Plan: record intended steps, tools, and files, e.g. {{"action":"set","summary":"...","steps":[{{"action":"Inspect code","tools":["Read"],"files":["src/main.rs"]}}]}}
Use Read before editing files you have not inspected. Prefer Edit for precise changes and Write for new files.
</tool_instructions>

{observation_prompt}

{mcp_prompt}

{goal_prompt}"#,
            workspace = self.workspace.display(),
            observation_prompt = ObservationSystem::system_prompt(),
            mcp_prompt = self.mcp.system_prompt(),
            goal_prompt = self.goals.system_prompt()
        )
    }

    pub fn command_output(&mut self, input: &str) -> Result<Option<String>> {
        if let Some(output) = self.observation_command_output(input)? {
            return Ok(Some(output));
        }
        if let Some(output) = self.computer_use.command_output(input)? {
            return Ok(Some(output));
        }
        if let Some(output) = self.mcp.command_output(input)? {
            return Ok(Some(output));
        }
        if let Some(output) = self.skills.command_output(input)? {
            return Ok(Some(output));
        }
        self.goals.command_output(input)
    }

    pub async fn execute(&mut self, client: &Client, call: ToolCall) -> Result<String> {
        let tool_name = call.tool.clone();
        let before = self.observations.before_action();
        let result = match call.tool.to_lowercase().as_str() {
            "goal" => self.goals.execute_goal(&call.args),
            "plan" => self.goals.execute_plan(&call.args),
            "observe" | "observation" => self.observe(client, &call.args).await,
            "read" => self.read(&call.args),
            "write" => self.write(&call.args),
            "edit" => self.edit(&call.args),
            "bash" => self.bash(&call.args),
            "monitor" => self.monitor(&call.args),
            "mcp" => self.mcp.execute(&call.args),
            "computeruse" | "computer_use" | "computer-use" | "computer use" => {
                self.computer_use.execute(&call.args)
            }
            "glob" => self.glob(&call.args),
            "grep" => self.grep(&call.args),
            "websearch" => self.web_search(client, &call.args).await,
            "webfetch" => self.web_fetch(client, &call.args).await,
            "askuserquestion" => self.ask_user(&call.args),
            other => bail!("unknown tool: {other}"),
        }?;
        Ok(self.observations.after_action(&tool_name, result, before))
    }

    fn observation_command_output(&mut self, input: &str) -> Result<Option<String>> {
        let output = match input {
            "/observe" | "/observe screen" => self
                .observations
                .observe_screen(&self.active_monitor_summary()),
            "/observe workspace" => self.observations.observe_workspace(),
            "/observe changes" => self.observations.observe_changes(),
            "/observe terminal" => self.observations.observe_terminal(),
            _ => return Ok(None),
        };
        Ok(Some(output))
    }

    async fn observe(&mut self, client: &Client, args: &Value) -> Result<String> {
        match crate::observations::target_arg(args)
            .to_lowercase()
            .as_str()
        {
            "workspace" | "files" => Ok(self.observations.observe_workspace()),
            "changes" | "diff" => Ok(self.observations.observe_changes()),
            "file" => {
                let path = str_arg(args, "path")?;
                let max = usize_arg(args, "max_bytes", 12_000);
                self.observations.observe_file(path, max)
            }
            "terminal" | "shell" => Ok(self.observations.observe_terminal()),
            "monitor" => self.observe_monitor(args),
            "web" | "page" | "webpage" => self.observe_webpage(client, args).await,
            "screen" | "state" => Ok(self
                .observations
                .observe_screen(&self.active_monitor_summary())),
            other => bail!("unknown Observe target: {other}"),
        }
    }

    fn observe_monitor(&mut self, args: &Value) -> Result<String> {
        let id = u64_arg(args, "id", 0);
        if id == 0 {
            return Ok(self.active_monitor_summary());
        }
        let monitor = self.monitors.get_mut(&id).context("monitor not found")?;
        let output = read_monitor(id, monitor)?;
        self.observations.remember_terminal(output.clone());
        Ok(output)
    }

    async fn observe_webpage(&mut self, client: &Client, args: &Value) -> Result<String> {
        let url = str_arg(args, "url")?;
        let max = usize_arg(args, "max_chars", 12_000);
        let response = client
            .get(url)
            .header(USER_AGENT, concat!("biscuits/", env!("CARGO_PKG_VERSION")))
            .send()
            .await?;
        let final_url = response.url().to_string();
        let body = response.text().await?;
        let text = truncate(&strip_html(&body), max);
        Ok(self.observations.observe_webpage(&final_url, &text))
    }

    fn active_monitor_summary(&self) -> String {
        self.monitors
            .iter()
            .map(|(id, monitor)| format!("{id}: {}", monitor.command))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn read(&self, args: &Value) -> Result<String> {
        let path = self.existing_path(str_arg(args, "path")?)?;
        self.ensure_not_biscuit_log(&path)?;
        let max = usize_arg(args, "max_bytes", 20_000);
        let bytes = fs::read(&path)?;
        let text = String::from_utf8_lossy(&bytes);
        Ok(format!(
            "Read {} ({} bytes)\n{}",
            rel(&self.workspace, &path),
            bytes.len(),
            truncate(&text, max)
        ))
    }

    fn write(&self, args: &Value) -> Result<String> {
        let path = self.new_path(str_arg(args, "path")?)?;
        self.ensure_not_biscuit_log(&path)?;
        let content = str_arg(args, "content")?;
        let overwrite = bool_arg(args, "overwrite", false);
        if path.exists() && !overwrite {
            bail!("file exists; pass overwrite=true or use Edit");
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        Ok(format!(
            "Wrote {} ({} bytes)",
            rel(&self.workspace, &path),
            content.len()
        ))
    }

    fn edit(&self, args: &Value) -> Result<String> {
        let path = self.existing_path(str_arg(args, "path")?)?;
        self.ensure_not_biscuit_log(&path)?;
        let old = str_arg(args, "old")?;
        let new = str_arg(args, "new")?;
        let replace_all = bool_arg(args, "replace_all", false);
        let text = fs::read_to_string(&path)?;
        let count = text.matches(old).count();
        if count == 0 {
            bail!("old text not found");
        }
        if count > 1 && !replace_all {
            bail!("old text matched {count} times; pass replace_all=true or make it more precise");
        }
        let updated = if replace_all {
            text.replace(old, new)
        } else {
            text.replacen(old, new, 1)
        };
        fs::write(&path, updated)?;
        Ok(format!(
            "Edited {} ({} replacement(s))",
            rel(&self.workspace, &path),
            count
        ))
    }

    fn bash(&self, args: &Value) -> Result<String> {
        let command = str_arg(args, "command")?;
        let timeout = u64_arg(args, "timeout_secs", 60).min(600);
        let mut child = crate::shell::command(command)
            .current_dir(&self.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to run command: {command}"))?;

        let start = Instant::now();
        let mut timed_out = false;
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if start.elapsed() > Duration::from_secs(timeout) {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        let output = child.wait_with_output()?;
        Ok(format!(
            "command: {command}\nstatus: {}\ntimed_out: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            timed_out,
            truncate(&String::from_utf8_lossy(&output.stdout), 16_000),
            truncate(&String::from_utf8_lossy(&output.stderr), 16_000)
        ))
    }

    fn monitor(&mut self, args: &Value) -> Result<String> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                if args.get("command").is_some() {
                    "start"
                } else {
                    "read"
                }
            });

        match action {
            "start" => {
                let command = str_arg(args, "command")?.to_string();
                let mut child = crate::shell::command(&command)
                    .current_dir(&self.workspace)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .with_context(|| format!("failed to start monitor: {command}"))?;
                let (tx, rx) = mpsc::channel();
                if let Some(stdout) = child.stdout.take() {
                    let tx = tx.clone();
                    thread::spawn(move || {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            let _ = tx.send(format!("stdout: {line}"));
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    thread::spawn(move || {
                        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                            let _ = tx.send(format!("stderr: {line}"));
                        }
                    });
                }
                let id = self.next_monitor;
                self.next_monitor += 1;
                self.monitors.insert(
                    id,
                    Monitor {
                        command,
                        child,
                        rx,
                        buffer: Vec::new(),
                    },
                );
                Ok(format!("monitor {id} started"))
            }
            "read" => {
                let id = u64_arg(args, "id", 0);
                if id == 0 {
                    let ids = self
                        .monitors
                        .iter()
                        .map(|(id, m)| format!("{id}: {}", m.command))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(if ids.is_empty() {
                        "no monitors".into()
                    } else {
                        ids
                    });
                }
                let monitor = self.monitors.get_mut(&id).context("monitor not found")?;
                Ok(read_monitor(id, monitor)?)
            }
            "stop" => {
                let id = u64_arg(args, "id", 0);
                let mut monitor = self.monitors.remove(&id).context("monitor not found")?;
                let _ = monitor.child.kill();
                Ok(format!(
                    "monitor {id} stopped\n{}",
                    read_monitor(id, &mut monitor)?
                ))
            }
            _ => bail!("unknown monitor action: {action}"),
        }
    }

    fn glob(&self, args: &Value) -> Result<String> {
        let pattern = str_arg(args, "pattern")?;
        if Path::new(pattern).is_absolute() {
            bail!("glob pattern must be relative to workspace");
        }
        let full = self.workspace.join(pattern);
        let mut out = Vec::new();
        for entry in glob(&full.to_string_lossy())? {
            let path = entry?;
            if path.is_file()
                && inside(&self.workspace, &path)
                && !is_biscuit_log(&self.workspace, &path)
            {
                out.push(rel(&self.workspace, &path));
            }
            if out.len() >= 500 {
                break;
            }
        }
        Ok(out.join("\n"))
    }

    fn grep(&self, args: &Value) -> Result<String> {
        let re = Regex::new(str_arg(args, "pattern")?)?;
        let root = self.existing_path(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
        let glob_pat = args.get("glob").and_then(Value::as_str);
        let matcher = glob_pat.map(Pattern::new).transpose()?;
        let max = usize_arg(args, "max_matches", 100);
        let mut files = Vec::new();
        collect_files(&root, &self.workspace, &mut files)?;
        let mut hits = Vec::new();
        for file in files {
            let rel_path = rel(&self.workspace, &file);
            if let Some(matcher) = &matcher {
                if !matcher.matches(&rel_path) {
                    continue;
                }
            }
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(format!("{}:{}: {}", rel_path, i + 1, line.trim()));
                    if hits.len() >= max {
                        return Ok(hits.join("\n"));
                    }
                }
            }
        }
        Ok(hits.join("\n"))
    }

    async fn web_search(&self, client: &Client, args: &Value) -> Result<String> {
        let query = str_arg(args, "query")?;
        let limit = usize_arg(args, "limit", 5).min(10);
        let url = format!(
            "https://duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );
        let html = client
            .get(url)
            .header(USER_AGENT, concat!("biscuits/", env!("CARGO_PKG_VERSION")))
            .send()
            .await?
            .text()
            .await?;
        let re = Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)?;
        let mut out = Vec::new();
        for cap in re.captures_iter(&html).take(limit) {
            let url = clean_url(&html_unescape(&cap[1]));
            let title = strip_html(&cap[2]);
            out.push(format!("- {title}\n  {url}"));
        }
        if out.is_empty() {
            Ok("no web search results parsed".into())
        } else {
            Ok(out.join("\n"))
        }
    }

    async fn web_fetch(&self, client: &Client, args: &Value) -> Result<String> {
        let url = str_arg(args, "url")?;
        let max = usize_arg(args, "max_chars", 12_000);
        let response = client
            .get(url)
            .header(USER_AGENT, concat!("biscuits/", env!("CARGO_PKG_VERSION")))
            .send()
            .await?;
        let final_url = response.url().to_string();
        let body = response.text().await?;
        Ok(format!(
            "url: {final_url}\n\n{}",
            truncate(&strip_html(&body), max)
        ))
    }

    fn ask_user(&self, args: &Value) -> Result<String> {
        let question = str_arg(args, "question")?;
        println!("\nbiscuits asks> {question}");
        let choices: Vec<_> = args
            .get("choices")
            .and_then(Value::as_array)
            .map(|xs| xs.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for (i, choice) in choices.iter().enumerate() {
            println!("  {}. {}", i + 1, choice);
        }
        print!("answer> ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let answer = input.trim();
        if let Ok(n) = answer.parse::<usize>() {
            if let Some(choice) = choices.get(n.saturating_sub(1)) {
                return Ok(format!("user selected: {choice}"));
            }
        }
        Ok(format!("user answered: {answer}"))
    }

    fn existing_path(&self, path: &str) -> Result<PathBuf> {
        let path = self.workspace.join(path).canonicalize()?;
        if !path.starts_with(&self.workspace) {
            bail!("path is outside workspace");
        }
        Ok(path)
    }

    fn ensure_not_biscuit_log(&self, path: &Path) -> Result<()> {
        if is_biscuit_log(&self.workspace, path) {
            bail!("biscuit logs are runtime-maintained and not readable by the agent");
        }
        Ok(())
    }

    fn new_path(&self, path: &str) -> Result<PathBuf> {
        let path = self.workspace.join(path);
        let parent = path.parent().unwrap_or(&self.workspace);
        let mut existing = parent;
        while !existing.exists() {
            existing = existing.parent().context("path has no existing parent")?;
        }
        if !existing.canonicalize()?.starts_with(&self.workspace) {
            bail!("path is outside workspace");
        }
        Ok(path)
    }
}

impl Drop for ToolRuntime {
    fn drop(&mut self) {
        for monitor in self.monitors.values_mut() {
            let _ = monitor.child.kill();
        }
    }
}

pub fn parse_calls(text: &str) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::new();
    let re = Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>")?;
    for cap in re.captures_iter(text) {
        calls.push(json_to_call(&cap[1])?);
    }
    if !calls.is_empty() {
        return Ok(calls);
    }

    let re = Regex::new(r"(?s)<tool_calls>\s*(.*?)\s*</tool_calls>")?;
    if let Some(cap) = re.captures(text) {
        let v: Value = serde_json::from_str(cap[1].trim())?;
        if let Some(xs) = v.as_array() {
            for x in xs {
                calls.push(value_to_call(x.clone())?);
            }
        }
    }
    if !calls.is_empty() {
        return Ok(calls);
    }

    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.contains("\"tool\"") {
        calls.push(json_to_call(trimmed)?);
    }
    Ok(calls)
}

pub fn brief(text: &str) -> String {
    let lines = text.lines().take(12).collect::<Vec<_>>().join("\n");
    truncate(&lines, 1200)
}

fn json_to_call(text: &str) -> Result<ToolCall> {
    value_to_call(serde_json::from_str(text.trim())?)
}

fn value_to_call(value: Value) -> Result<ToolCall> {
    let tool = value
        .get("tool")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .context("tool call missing tool/name")?
        .to_string();
    let args = value.get("args").cloned().unwrap_or(Value::Null);
    Ok(ToolCall { tool, args })
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string arg: {key}"))
}

fn bool_arg(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn usize_arg(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(default)
}

fn u64_arg(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn read_monitor(id: u64, monitor: &mut Monitor) -> Result<String> {
    while let Ok(line) = monitor.rx.try_recv() {
        monitor.buffer.push(line);
    }
    let status = match monitor.child.try_wait()? {
        Some(status) => format!("exited: {status}"),
        None => "running".into(),
    };
    let output = if monitor.buffer.is_empty() {
        "(no new output)".into()
    } else {
        let text = monitor.buffer.join("\n");
        monitor.buffer.clear();
        text
    };
    Ok(format!(
        "monitor {id}\ncommand: {}\nstatus: {status}\noutput:\n{}",
        monitor.command,
        truncate(&output, 16_000)
    ))
}

fn collect_files(root: &Path, workspace: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if root.is_file() {
        if !is_biscuit_log(workspace, root) {
            out.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if matches!(
            name,
            "target" | ".git" | "node_modules" | ".next" | "dist" | "build"
        ) || is_biscuit_log(workspace, &path)
        {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, workspace, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn inside(workspace: &Path, path: &Path) -> bool {
    path.canonicalize()
        .map(|p| p.starts_with(workspace))
        .unwrap_or(false)
}

fn is_biscuit_log(workspace: &Path, path: &Path) -> bool {
    let rel = rel(workspace, path);
    rel.strip_prefix("biscuit/")
        .and_then(|name| name.strip_prefix("logs"))
        .map(|suffix| {
            suffix == ".md"
                || (suffix.len() >= 3
                    && suffix.ends_with(".md")
                    && suffix[..suffix.len() - 3]
                        .chars()
                        .all(|c| c.is_ascii_digit()))
        })
        .unwrap_or(false)
}

fn rel(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        // Normalize to forward slashes so is_biscuit_log and downstream matching
        // behave the same on Windows (where strip_prefix yields backslashes).
        .replace('\\', "/")
}

fn strip_html(input: &str) -> String {
    let mut s = input.to_string();
    for pat in [
        r"(?is)<script.*?</script>",
        r"(?is)<style.*?</style>",
        r"(?s)<[^>]+>",
        r"\s+",
    ] {
        if let Ok(re) = Regex::new(pat) {
            s = re.replace_all(&s, " ").to_string();
        }
    }
    html_unescape(&s).trim().to_string()
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn clean_url(url: &str) -> String {
    if let Some(pos) = url.find("uddg=") {
        let part = &url[pos + 5..];
        let end = part.find('&').unwrap_or(part.len());
        return urlencoding::decode(&part[..end])
            .unwrap_or_else(|_| part[..end].into())
            .to_string();
    }
    url.to_string()
}

fn one_line(text: &str, max: usize) -> String {
    truncate(&text.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}
