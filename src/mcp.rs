use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CLIENT_PROTOCOL_VERSION: &str = "2024-11-05";
/// Cap a server-declared Content-Length so a buggy/hostile header can't trigger
/// a multi-gigabyte allocation that aborts the whole CLI.
const MAX_MCP_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

pub struct McpManager {
    workspace: PathBuf,
    config_path: PathBuf,
    servers: HashMap<String, McpServerConfig>,
    clients: HashMap<String, McpClient>,
}

#[derive(Default, Deserialize, Serialize)]
struct McpConfigFile {
    servers: Vec<McpServerConfig>,
}

#[derive(Clone, Deserialize, Serialize)]
struct McpServerConfig {
    name: String,
    command: String,
    auto_start: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct McpToolInfo {
    name: String,
    description: String,
    input_schema: Value,
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    stderr_rx: Receiver<String>,
    stderr: Vec<String>,
    next_id: u64,
    protocol_version: String,
    tools: Vec<McpToolInfo>,
}

impl McpManager {
    pub fn open(workspace: &Path) -> Result<Self> {
        let root = workspace.join(".biscuits");
        fs::create_dir_all(&root)?;
        let config_path = root.join("mcp_servers.json");
        let config = if config_path.exists() {
            let text = fs::read_to_string(&config_path)?;
            serde_json::from_str::<McpConfigFile>(&text).unwrap_or_default()
        } else {
            McpConfigFile::default()
        };

        let mut servers = HashMap::new();
        for server in config.servers {
            if !server.name.trim().is_empty() && !server.command.trim().is_empty() {
                servers.insert(server.name.clone(), server);
            }
        }

        let manager = Self {
            workspace: workspace.to_path_buf(),
            config_path,
            servers,
            clients: HashMap::new(),
        };
        manager.save()?;
        Ok(manager)
    }

    pub fn system_prompt(&self) -> String {
        let servers = self.render_servers(false);
        format!(
            r#"<mcp_system>
MCP servers can be connected and used with the Mcp tool. If the user asks you to connect an MCP server and provides enough package/command detail, do it for them.
Mcp tool examples:
<tool_call>{{"tool":"Mcp","args":{{"action":"connect","name":"filesystem","command":"npx -y @modelcontextprotocol/server-filesystem ."}}}}</tool_call>
<tool_call>{{"tool":"Mcp","args":{{"action":"tools","server":"filesystem"}}}}</tool_call>
<tool_call>{{"tool":"Mcp","args":{{"action":"call","server":"filesystem","name":"read_file","arguments":{{"path":"README.md"}}}}}}</tool_call>

Current MCP servers:
{servers}
</mcp_system>"#
        )
    }

    pub fn command_output(&mut self, input: &str) -> Result<Option<String>> {
        let Some(rest) = input.strip_prefix("/mcp") else {
            return Ok(None);
        };
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return Ok(None);
        }

        let rest = rest.trim();
        if rest.is_empty() || rest == "help" {
            return Ok(Some(self.help()));
        }

        let (action, rest) = next_word(rest);
        let output = match action {
            "add" | "connect" => {
                let (name, command) = next_word(rest);
                if name.is_empty() || command.trim().is_empty() {
                    bail!("usage: /mcp connect <name> [--] <command>");
                }
                self.connect(name, trim_command_marker(command), true)?
            }
            "start" => {
                let name = required_word(rest, "usage: /mcp start <name>")?;
                self.start(name)?
            }
            "stop" | "disconnect" => {
                let name = required_word(rest, "usage: /mcp stop <name>")?;
                self.stop(name)?
            }
            "remove" | "rm" => {
                let name = required_word(rest, "usage: /mcp remove <name>")?;
                self.remove(name)?
            }
            "list" | "status" => self.render_servers(true),
            "tools" => {
                let server = rest.trim();
                let server = if server.is_empty() {
                    None
                } else {
                    Some(server)
                };
                self.list_tools(server)?
            }
            "call" => {
                let (server, rest) = next_word(rest);
                let (tool, args_text) = next_word(rest);
                if server.is_empty() || tool.is_empty() {
                    bail!("usage: /mcp call <server> <tool> [json-arguments]");
                }
                let arguments = parse_json_args(args_text)?;
                self.call_tool(server, tool, arguments)?
            }
            _ => bail!("unknown /mcp action: {action}"),
        };
        Ok(Some(output))
    }

    pub fn execute(&mut self, args: &Value) -> Result<String> {
        let action = args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("list")
            .to_lowercase();
        match action.as_str() {
            "add" | "connect" => {
                let name = str_arg_any(args, &["name", "server"])?;
                let command = str_arg(args, "command")?;
                let auto_start = args
                    .get("auto_start")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                self.connect(name, command, auto_start)
            }
            "start" => {
                let name = str_arg_any(args, &["name", "server"])?;
                self.start(name)
            }
            "stop" | "disconnect" => {
                let name = str_arg_any(args, &["name", "server"])?;
                self.stop(name)
            }
            "remove" | "rm" => {
                let name = str_arg_any(args, &["name", "server"])?;
                self.remove(name)
            }
            "list" | "status" => Ok(self.render_servers(true)),
            "tools" => {
                let server = args
                    .get("server")
                    .or_else(|| args.get("name"))
                    .and_then(Value::as_str);
                self.list_tools(server)
            }
            "call" | "tool" => {
                let server = str_arg_any(args, &["server", "mcp", "mcp_server"])?;
                let tool = str_arg_any(args, &["name", "tool", "tool_name"])?;
                let arguments = args
                    .get("arguments")
                    .or_else(|| args.get("args"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                self.call_tool(server, tool, arguments)
            }
            _ => bail!("unknown Mcp action: {action}"),
        }
    }

    fn help(&self) -> String {
        format!(
            r#"MCP commands:
  /mcp                         show this help
  /mcp list                    list configured MCP servers
  /mcp connect <name> -- <cmd> add and start a stdio MCP server
  /mcp start <name>            start a configured server
  /mcp stop <name>             stop a running server
  /mcp remove <name>           stop and remove a configured server
  /mcp tools [name]            list MCP tools
  /mcp call <server> <tool> {{}} call an MCP tool with JSON arguments

Configured servers:
{}"#,
            self.render_servers(false)
        )
    }

    fn connect(&mut self, name: &str, command: &str, auto_start: bool) -> Result<String> {
        let name = clean_name(name)?;
        let command = command.trim();
        if command.is_empty() {
            bail!("MCP command cannot be empty");
        }

        self.servers.insert(
            name.clone(),
            McpServerConfig {
                name: name.clone(),
                command: command.to_string(),
                auto_start,
            },
        );
        self.save()?;
        if auto_start {
            let status = self.start(&name)?;
            Ok(format!("MCP server '{name}' connected\n{status}"))
        } else {
            Ok(format!("MCP server '{name}' connected"))
        }
    }

    fn start(&mut self, name: &str) -> Result<String> {
        let name = clean_name(name)?;
        if let Some(client) = self.clients.get_mut(&name) {
            // If the existing process has already exited, drop the dead client
            // and start fresh — otherwise it stays wedged as "already running"
            // forever and every tool call fails with no recovery path.
            match client.child.try_wait() {
                Ok(Some(_)) => {
                    if let Some(mut dead) = self.clients.remove(&name) {
                        let _ = dead.child.wait();
                    }
                }
                _ => return Ok(format!("MCP server '{name}' is already running")),
            }
        }
        let config = self
            .servers
            .get(&name)
            .with_context(|| format!("MCP server not configured: {name}"))?
            .clone();

        let mut child = crate::shell::command(&config.command)
            .current_dir(&self.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start MCP server '{name}': {}", config.command))?;

        let stdin = child.stdin.take().context("MCP server stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("MCP server stdout unavailable")?;
        let stderr = child
            .stderr
            .take()
            .context("MCP server stderr unavailable")?;
        let (tx, rx) = mpsc::channel();
        let (stderr_tx, stderr_rx) = mpsc::channel();

        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        let _ = tx.send(message);
                    }
                    Ok(None) => break,
                    Err(err) => {
                        let _ = tx.send(json!({
                            "biscuits_mcp_error": err.to_string()
                        }));
                        break;
                    }
                }
            }
        });

        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = stderr_tx.send(line);
            }
        });

        let mut client = McpClient {
            child,
            stdin,
            rx,
            stderr_rx,
            stderr: Vec::new(),
            next_id: 1,
            protocol_version: CLIENT_PROTOCOL_VERSION.to_string(),
            tools: Vec::new(),
        };
        if let Err(err) = client.initialize(&name) {
            // Reap the child we just spawned so a failed handshake (e.g. a
            // timeout) doesn't leak a live, detached MCP server process.
            let _ = client.child.kill();
            let _ = client.child.wait();
            return Err(err);
        }
        let tool_count = client.tools.len();
        self.clients.insert(name.clone(), client);
        Ok(format!(
            "MCP server '{name}' started ({tool_count} tool(s) discovered)"
        ))
    }

    fn stop(&mut self, name: &str) -> Result<String> {
        let name = clean_name(name)?;
        let Some(mut client) = self.clients.remove(&name) else {
            return Ok(format!("MCP server '{name}' is not running"));
        };
        let _ = client.child.kill();
        let _ = client.child.wait();
        let stderr = client.stderr_tail();
        let suffix = if stderr.is_empty() {
            String::new()
        } else {
            format!("\nstderr:\n{stderr}")
        };
        Ok(format!("MCP server '{name}' stopped{suffix}"))
    }

    fn remove(&mut self, name: &str) -> Result<String> {
        let name = clean_name(name)?;
        let stopped = self.stop(&name)?;
        if self.servers.remove(&name).is_some() {
            self.save()?;
            Ok(format!("{stopped}\nMCP server '{name}' removed"))
        } else {
            Ok(format!("{stopped}\nMCP server '{name}' was not configured"))
        }
    }

    fn list_tools(&mut self, server: Option<&str>) -> Result<String> {
        if let Some(server) = server {
            let server = clean_name(server)?;
            self.ensure_started(&server)?;
            let client = self
                .clients
                .get_mut(&server)
                .with_context(|| format!("MCP server not running: {server}"))?;
            client.refresh_tools(&server)?;
            return Ok(format_tools(&server, &client.tools));
        }

        let mut names = self.servers.keys().cloned().collect::<Vec<_>>();
        names.sort();
        if names.is_empty() {
            return Ok("no configured MCP servers; use /mcp connect <name> -- <command>".into());
        }

        let mut out = Vec::new();
        for name in names {
            match self.ensure_started(&name) {
                Ok(()) => {
                    if let Some(client) = self.clients.get_mut(&name) {
                        let _ = client.refresh_tools(&name);
                        out.push(format_tools(&name, &client.tools));
                    }
                }
                Err(err) => out.push(format!("{name}: failed to start: {err}")),
            }
        }
        Ok(out.join("\n\n"))
    }

    fn call_tool(&mut self, server: &str, tool: &str, arguments: Value) -> Result<String> {
        let server = clean_name(server)?;
        self.ensure_started(&server)?;
        let client = self
            .clients
            .get_mut(&server)
            .with_context(|| format!("MCP server not running: {server}"))?;
        let result = client.call_tool(&server, tool, arguments)?;
        Ok(format!("MCP {server}.{tool} result:\n{result}"))
    }

    fn ensure_started(&mut self, name: &str) -> Result<()> {
        // start() is idempotent: it no-ops when the server is alive and restarts
        // it when the previous process has died.
        self.start(name)?;
        Ok(())
    }

    fn render_servers(&self, include_tools: bool) -> String {
        if self.servers.is_empty() {
            return "none".into();
        }

        let mut names = self.servers.keys().cloned().collect::<Vec<_>>();
        names.sort();
        let mut out = Vec::new();
        for name in names {
            let Some(config) = self.servers.get(&name) else {
                continue;
            };
            let status = if self.clients.contains_key(&name) {
                "running"
            } else {
                "stopped"
            };
            let mut line = format!("- {name}: {status}; command: {}", config.command);
            if include_tools {
                if let Some(client) = self.clients.get(&name) {
                    if !client.tools.is_empty() {
                        let tools = client
                            .tools
                            .iter()
                            .map(|tool| tool.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        line.push_str(&format!("; tools: {tools}"));
                    }
                }
            }
            out.push(line);
        }
        out.join("\n")
    }

    fn save(&self) -> Result<()> {
        let mut servers = self.servers.values().cloned().collect::<Vec<_>>();
        servers.sort_by(|a, b| a.name.cmp(&b.name));
        let config = McpConfigFile { servers };
        let mut file = fs::File::create(&self.config_path)?;
        file.write_all(serde_json::to_string_pretty(&config)?.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for client in self.clients.values_mut() {
            let _ = client.child.kill();
            let _ = client.child.wait();
        }
    }
}

impl McpClient {
    fn initialize(&mut self, server: &str) -> Result<()> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": CLIENT_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "biscuits",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        if let Some(version) = result.get("protocolVersion").and_then(Value::as_str) {
            self.protocol_version = version.to_string();
        }
        self.notify("notifications/initialized", json!({}))?;
        self.refresh_tools(server)?;
        Ok(())
    }

    fn refresh_tools(&mut self, server: &str) -> Result<()> {
        let result = self.request("tools/list", json!({}))?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.tools = tools
            .into_iter()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?.to_string();
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let input_schema = tool
                    .get("inputSchema")
                    .or_else(|| tool.get("input_schema"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Some(McpToolInfo {
                    name,
                    description,
                    input_schema,
                })
            })
            .collect();
        if self.tools.is_empty() {
            self.drain_stderr();
        }
        if self.child.try_wait()?.is_some() {
            bail!("MCP server '{server}' exited during tools/list");
        }
        Ok(())
    }

    fn call_tool(&mut self, server: &str, tool: &str, arguments: Value) -> Result<String> {
        let result = self.request(
            "tools/call",
            json!({
                "name": tool,
                "arguments": arguments
            }),
        )?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            bail!(
                "MCP tool '{server}.{tool}' returned an error: {}",
                format_result(&result)
            );
        }
        Ok(format_result(&result))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        write_message(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            }),
        )?;

        let start = Instant::now();
        while start.elapsed() < REQUEST_TIMEOUT {
            self.drain_stderr();
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(message) => {
                    if let Some(err) = message.get("biscuits_mcp_error") {
                        bail!("MCP protocol read error: {err}");
                    }
                    if response_id(&message) != Some(id) {
                        continue;
                    }
                    if let Some(error) = message.get("error") {
                        bail!("MCP request '{method}' failed: {error}");
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(status) = self.child.try_wait()? {
                        let stderr = self.stderr_tail();
                        bail!("MCP server exited while handling '{method}': {status}\n{stderr}");
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let stderr = self.stderr_tail();
                    bail!("MCP server output closed while handling '{method}'\n{stderr}");
                }
            }
        }

        let stderr = self.stderr_tail();
        bail!("MCP request '{method}' timed out after 20s\n{stderr}");
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        write_message(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params
            }),
        )
    }

    fn drain_stderr(&mut self) {
        while let Ok(line) = self.stderr_rx.try_recv() {
            self.stderr.push(line);
        }
        let keep = 80;
        if self.stderr.len() > keep {
            let drain = self.stderr.len() - keep;
            self.stderr.drain(0..drain);
        }
    }

    fn stderr_tail(&mut self) -> String {
        self.drain_stderr();
        let start = self.stderr.len().saturating_sub(20);
        self.stderr[start..].join("\n")
    }
}

fn read_message<R: Read>(reader: &mut BufReader<R>) -> Result<Option<Value>> {
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') {
            match serde_json::from_str(trimmed) {
                Ok(value) => return Ok(Some(value)),
                // A server diagnostic line that merely starts with '{' is not a
                // JSON-RPC message — skip it instead of killing the connection.
                Err(_) => continue,
            }
        }
        if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            let length = trimmed
                .split_once(':')
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .context("invalid MCP Content-Length header")?;
            if length > MAX_MCP_MESSAGE_BYTES {
                bail!("MCP message too large: {length} bytes");
            }
            loop {
                let mut header = String::new();
                let n = reader.read_line(&mut header)?;
                if n == 0 {
                    return Ok(None);
                }
                if header.trim().is_empty() {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body)?;
            let value = serde_json::from_slice(&body)?;
            return Ok(Some(value));
        }
    }
}

fn write_message<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    // The MCP stdio transport uses newline-delimited JSON, NOT LSP-style
    // Content-Length framing. serde_json::to_string emits a single line with no
    // embedded newlines, so one write + '\n' is a complete, spec-compliant
    // message that real SDK servers (filesystem, etc.) actually accept.
    let body = serde_json::to_string(value)?;
    writer.write_all(body.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn response_id(value: &Value) -> Option<u64> {
    value.get("id").and_then(|id| {
        id.as_u64()
            .or_else(|| id.as_str().and_then(|s| s.parse::<u64>().ok()))
    })
}

fn format_tools(server: &str, tools: &[McpToolInfo]) -> String {
    if tools.is_empty() {
        return format!("{server}: no tools discovered");
    }
    let mut out = format!("{server} tools:");
    for tool in tools {
        out.push_str(&format!("\n- {}", tool.name));
        if !tool.description.trim().is_empty() {
            out.push_str(&format!(": {}", one_line(&tool.description, 160)));
        }
    }
    out
}

fn format_result(result: &Value) -> String {
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        let mut out = Vec::new();
        for item in content {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                out.push(text.to_string());
            } else {
                out.push(serde_json::to_string_pretty(item).unwrap_or_else(|_| item.to_string()));
            }
        }
        if !out.is_empty() {
            return out.join("\n");
        }
    }
    serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
}

fn parse_json_args(text: &str) -> Result<Value> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_str(text)?)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string arg: {key}"))
}

fn str_arg_any<'a>(args: &'a Value, keys: &[&str]) -> Result<&'a str> {
    for key in keys {
        if let Some(value) = args.get(*key).and_then(Value::as_str) {
            return Ok(value);
        }
    }
    bail!("missing string arg: {}", keys.join("/"))
}

fn clean_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("MCP server name cannot be empty");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        bail!("MCP server names may only use letters, numbers, _, -, and .");
    }
    Ok(name.to_string())
}

fn next_word(input: &str) -> (&str, &str) {
    let input = input.trim_start();
    if input.is_empty() {
        return ("", "");
    }
    match input.find(char::is_whitespace) {
        Some(pos) => (&input[..pos], input[pos..].trim_start()),
        None => (input, ""),
    }
}

fn required_word<'a>(input: &'a str, usage: &str) -> Result<&'a str> {
    let (word, _) = next_word(input);
    if word.is_empty() {
        bail!("{usage}");
    }
    Ok(word)
}

fn trim_command_marker(command: &str) -> &str {
    command
        .trim_start()
        .strip_prefix("--")
        .map(str::trim_start)
        .unwrap_or_else(|| command.trim_start())
}

fn one_line(text: &str, max: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max {
        compact
    } else {
        format!("{}...", compact.chars().take(max).collect::<String>())
    }
}
