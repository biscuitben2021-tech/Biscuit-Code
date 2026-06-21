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
/// How long `/install biscuit-browser` waits for the browser to publish its
/// MCP discovery file before giving up and telling the user to launch it.
const BROWSER_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

pub struct McpManager {
    workspace: PathBuf,
    config_path: PathBuf,
    plugins_path: PathBuf,
    servers: HashMap<String, McpServerConfig>,
    clients: HashMap<String, McpClient>,
    /// Names of plugins the user explicitly installed (persisted across runs).
    installed_plugins: Vec<String>,
}

#[derive(Default, Deserialize, Serialize)]
struct McpConfigFile {
    servers: Vec<McpServerConfig>,
}

#[derive(Default, Deserialize, Serialize)]
struct PluginsFile {
    installed: Vec<String>,
}

fn default_transport() -> String {
    "stdio".to_string()
}

#[derive(Clone, Deserialize, Serialize)]
struct McpServerConfig {
    name: String,
    command: String,
    auto_start: bool,
    /// "stdio" (default, child process) or "http" (JSON-RPC over HTTP). Defaults
    /// keep older {name,command,auto_start} configs deserializing unchanged.
    #[serde(default = "default_transport")]
    transport: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct McpToolInfo {
    name: String,
    description: String,
    input_schema: Value,
}

/// The wire transport a connected client speaks. A client is EITHER a child
/// process (stdio) OR a remote HTTP endpoint, never both.
enum Transport {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        rx: Receiver<Value>,
        stderr_rx: Receiver<String>,
        stderr: Vec<String>,
    },
    Http {
        url: String,
        token: Option<String>,
    },
}

struct McpClient {
    transport: Transport,
    next_id: u64,
    protocol_version: String,
    tools: Vec<McpToolInfo>,
}

impl McpManager {
    pub fn open(workspace: &Path) -> Result<Self> {
        let root = workspace.join(".biscuits");
        fs::create_dir_all(&root)?;
        let config_path = root.join("mcp_servers.json");
        let plugins_path = root.join("plugins.json");
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

        let installed_plugins = if plugins_path.exists() {
            let text = fs::read_to_string(&plugins_path)?;
            serde_json::from_str::<PluginsFile>(&text)
                .map(|p| p.installed)
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let manager = Self {
            workspace: workspace.to_path_buf(),
            config_path,
            plugins_path,
            servers,
            clients: HashMap::new(),
            installed_plugins,
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
        if let Some(output) = self.plugin_command_output(input)? {
            return Ok(Some(output));
        }

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

    /// Handle the plugin install surface (`/plugins`, `/install`, `/uninstall`,
    /// `/plugin start|stop`). Uses the same prefix-with-word-boundary guard as
    /// the `/mcp` handler so `/installer` etc. don't false-match. Returns
    /// `Ok(None)` when the input is not a plugin command.
    fn plugin_command_output(&mut self, input: &str) -> Result<Option<String>> {
        if let Some(rest) = command_rest(input, "/plugins") {
            if !rest.trim().is_empty() {
                bail!("usage: /plugins");
            }
            return Ok(Some(self.render_plugins()));
        }
        if let Some(rest) = command_rest(input, "/install") {
            let name = required_word(rest, "usage: /install <name>")?;
            return Ok(Some(self.install_plugin(name)?));
        }
        if let Some(rest) = command_rest(input, "/uninstall") {
            let name = required_word(rest, "usage: /uninstall <name>")?;
            return Ok(Some(self.uninstall_plugin(name)?));
        }
        if let Some(rest) = command_rest(input, "/plugin") {
            let (action, rest) = next_word(rest.trim());
            return match action {
                "start" => {
                    let name = required_word(rest, "usage: /plugin start <name>")?;
                    Ok(Some(self.start(name)?))
                }
                "stop" => {
                    let name = required_word(rest, "usage: /plugin stop <name>")?;
                    Ok(Some(self.stop(name)?))
                }
                "" => Ok(Some(self.render_plugins())),
                other => bail!("unknown /plugin action: {other}"),
            };
        }
        Ok(None)
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

Plugin commands:
  /plugins                     list built-in + installed plugins
  /install <name>              install and start a plugin
  /uninstall <name>            stop and remove a plugin
  /plugin start <name>         start an installed plugin
  /plugin stop <name>          stop a running plugin

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
                transport: default_transport(),
                url: None,
                token: None,
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

    /// Register (or replace) an HTTP MCP server config without starting it.
    fn register_http(&mut self, name: &str, url: &str, token: Option<String>) -> Result<()> {
        let name = clean_name(name)?;
        self.servers.insert(
            name.clone(),
            McpServerConfig {
                name,
                command: url.to_string(),
                auto_start: true,
                transport: "http".to_string(),
                url: Some(url.to_string()),
                token,
            },
        );
        self.save()
    }

    fn start(&mut self, name: &str) -> Result<String> {
        let name = clean_name(name)?;
        if let Some(client) = self.clients.get_mut(&name) {
            // If an existing stdio process has already exited, drop the dead
            // client and start fresh — otherwise it stays wedged as "already
            // running" forever and every tool call fails with no recovery path.
            // HTTP clients are stateless and always considered alive while held.
            if client.is_alive() {
                return Ok(format!("MCP server '{name}' is already running"));
            }
            self.clients.remove(&name);
        }
        let config = self
            .servers
            .get(&name)
            .with_context(|| format!("MCP server not configured: {name}"))?
            .clone();

        if config.transport == "http" {
            return self.start_http(&name, &config);
        }
        self.start_stdio(&name, &config)
    }

    fn start_stdio(&mut self, name: &str, config: &McpServerConfig) -> Result<String> {
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
            transport: Transport::Stdio {
                child,
                stdin,
                rx,
                stderr_rx,
                stderr: Vec::new(),
            },
            next_id: 1,
            protocol_version: CLIENT_PROTOCOL_VERSION.to_string(),
            tools: Vec::new(),
        };
        if let Err(err) = client.initialize(name) {
            // Reap the child we just spawned so a failed handshake (e.g. a
            // timeout) doesn't leak a live, detached MCP server process.
            client.kill();
            return Err(err);
        }
        let tool_count = client.tools.len();
        self.clients.insert(name.to_string(), client);
        Ok(format!(
            "MCP server '{name}' started ({tool_count} tool(s) discovered)"
        ))
    }

    fn start_http(&mut self, name: &str, config: &McpServerConfig) -> Result<String> {
        let url = config
            .url
            .clone()
            .or_else(|| {
                if config.command.starts_with("http") {
                    Some(config.command.clone())
                } else {
                    None
                }
            })
            .with_context(|| format!("HTTP MCP server '{name}' has no url"))?;
        let mut client = McpClient {
            transport: Transport::Http {
                url,
                token: config.token.clone(),
            },
            next_id: 1,
            protocol_version: CLIENT_PROTOCOL_VERSION.to_string(),
            tools: Vec::new(),
        };
        client.initialize(name)?;
        let tool_count = client.tools.len();
        self.clients.insert(name.to_string(), client);
        Ok(format!(
            "MCP server '{name}' started ({tool_count} tool(s) discovered)"
        ))
    }

    fn stop(&mut self, name: &str) -> Result<String> {
        let name = clean_name(name)?;
        let Some(mut client) = self.clients.remove(&name) else {
            return Ok(format!("MCP server '{name}' is not running"));
        };
        client.kill();
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
            let detail = if config.transport == "http" {
                format!("http: {}", config.url.as_deref().unwrap_or(&config.command))
            } else {
                format!("command: {}", config.command)
            };
            let mut line = format!("- {name}: {status}; {detail}");
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

    // ── Plugin install system ──────────────────────────────────────────────

    fn render_plugins(&self) -> String {
        let mut out = vec!["Plugins:".to_string()];
        for plugin in plugin_registry() {
            let installed = self.installed_plugins.iter().any(|n| n == plugin.name);
            let running = self.clients.contains_key(plugin.name);
            let state = if running {
                "running"
            } else if installed {
                "installed (stopped)"
            } else {
                "available"
            };
            out.push(format!(
                "- {} [{}]: {} ({})",
                plugin.name, plugin.transport, plugin.description, state
            ));
        }
        // Surface any installed plugin names that are not built-ins (forward
        // compatibility if the registry ever changes between versions).
        for name in &self.installed_plugins {
            if plugin_registry().iter().all(|p| p.name != name) {
                let running = self.clients.contains_key(name);
                let state = if running {
                    "running"
                } else {
                    "installed (stopped)"
                };
                out.push(format!("- {name} (unknown plugin) ({state})"));
            }
        }
        out.join("\n")
    }

    fn install_plugin(&mut self, name: &str) -> Result<String> {
        let plugin = plugin_registry()
            .into_iter()
            .find(|p| p.name == name)
            .with_context(|| {
                format!("unknown plugin '{name}'; run /plugins to list available plugins")
            })?;

        let started = match plugin.kind {
            PluginKind::BiscuitBrowser => self.install_biscuit_browser(&plugin)?,
            PluginKind::BiscuitGui => self.install_biscuit_gui(&plugin)?,
        };

        if !self.installed_plugins.iter().any(|n| n == plugin.name) {
            self.installed_plugins.push(plugin.name.to_string());
            self.save_plugins()?;
        }
        Ok(format!("Plugin '{}' installed.\n{started}", plugin.name))
    }

    fn install_biscuit_gui(&mut self, plugin: &Plugin) -> Result<String> {
        let command = resolve_biscuit_gui_command(&self.workspace);
        // Register as a normal stdio server and start it through the existing
        // spawn path so its tools are immediately usable.
        self.connect(plugin.name, &command, true)
    }

    fn install_biscuit_browser(&mut self, plugin: &Plugin) -> Result<String> {
        let browser_dir = self.workspace.join("biscuit-browser");
        if !browser_dir.is_dir() {
            bail!(
                "biscuit-browser/ not found in workspace; clone or place the browser app there first"
            );
        }

        // 1. npm install if node_modules is missing.
        if !browser_dir.join("node_modules").exists() {
            let output = crate::shell::command("npm install")
                .current_dir(&browser_dir)
                .output()
                .context("failed to run `npm install` in biscuit-browser/")?;
            if !output.status.success() {
                bail!(
                    "`npm install` in biscuit-browser/ failed:\n{}",
                    truncate(&String::from_utf8_lossy(&output.stderr), 2000)
                );
            }
        }

        // 2. Launch `npm run dev` detached, in its own process group, with null
        // stdio so it survives independently and doesn't pipe into the CLI.
        let mut cmd = crate::shell::command("npm run dev");
        cmd.current_dir(&browser_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::shell::spawn_in_own_group(&mut cmd);
        cmd.spawn()
            .context("failed to launch `npm run dev` for biscuit-browser")?;

        // 3. Poll the discovery file the browser writes on MCP start.
        let discovery = browser_discovery_path()?;
        let endpoint = poll_browser_discovery(&discovery, BROWSER_DISCOVERY_TIMEOUT)?;

        // 4. Register + start the HTTP MCP server.
        self.register_http(plugin.name, &endpoint.url, endpoint.token)?;
        self.start(plugin.name)
    }

    fn uninstall_plugin(&mut self, name: &str) -> Result<String> {
        // Accept any registered/installed name (clean for safety).
        let name = clean_name(name)?;
        let stopped = self.stop(&name)?;
        let was_configured = self.servers.remove(&name).is_some();
        if was_configured {
            self.save()?;
        }
        let was_installed =
            if let Some(pos) = self.installed_plugins.iter().position(|n| n == &name) {
                self.installed_plugins.remove(pos);
                self.save_plugins()?;
                true
            } else {
                false
            };
        if !was_installed && !was_configured {
            bail!("plugin '{name}' is not installed");
        }
        Ok(format!("{stopped}\nPlugin '{name}' uninstalled"))
    }

    fn save_plugins(&self) -> Result<()> {
        let mut installed = self.installed_plugins.clone();
        installed.sort();
        installed.dedup();
        let payload = PluginsFile { installed };
        let text = serde_json::to_string_pretty(&payload)?;
        // Atomic write: temp file in the same dir + rename.
        let tmp = self.plugins_path.with_extension("json.tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.write_all(b"\n")?;
            file.flush()?;
        }
        fs::rename(&tmp, &self.plugins_path)?;
        Ok(())
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        for client in self.clients.values_mut() {
            client.kill();
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
        if !self.is_alive() {
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
        match &mut self.transport {
            Transport::Stdio { .. } => self.request_stdio(id, method, params),
            Transport::Http { url, token } => {
                let url = url.clone();
                let token = token.clone();
                request_http(&url, token.as_deref(), id, method, params)
            }
        }
    }

    fn request_stdio(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let Transport::Stdio {
            child,
            stdin,
            rx,
            stderr,
            stderr_rx,
        } = &mut self.transport
        else {
            unreachable!("request_stdio called on non-stdio transport");
        };
        write_message(stdin, &body)?;

        let start = Instant::now();
        while start.elapsed() < REQUEST_TIMEOUT {
            drain_stderr_into(stderr_rx, stderr);
            match rx.recv_timeout(Duration::from_millis(100)) {
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
                    if let Some(status) = child.try_wait()? {
                        let tail = tail_lines(stderr);
                        bail!("MCP server exited while handling '{method}': {status}\n{tail}");
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let tail = tail_lines(stderr);
                    bail!("MCP server output closed while handling '{method}'\n{tail}");
                }
            }
        }

        let tail = tail_lines(stderr);
        bail!("MCP request '{method}' timed out after 20s\n{tail}");
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        match &mut self.transport {
            Transport::Stdio { stdin, .. } => write_message(
                stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "method": method,
                    "params": params
                }),
            ),
            Transport::Http { url, token } => {
                let url = url.clone();
                let token = token.clone();
                // Fire a notification (no id) and ignore the response/body.
                let _ = notify_http(&url, token.as_deref(), method, params);
                Ok(())
            }
        }
    }

    /// Whether the underlying transport is still usable. Stdio is alive while
    /// the child process has not exited; HTTP is stateless and is treated as
    /// alive for as long as the client object is held.
    fn is_alive(&mut self) -> bool {
        match &mut self.transport {
            Transport::Stdio { child, .. } => !matches!(child.try_wait(), Ok(Some(_))),
            Transport::Http { .. } => true,
        }
    }

    /// Stop the transport: kill+reap a child process; HTTP is a no-op.
    fn kill(&mut self) {
        if let Transport::Stdio { child, .. } = &mut self.transport {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn drain_stderr(&mut self) {
        if let Transport::Stdio {
            stderr_rx, stderr, ..
        } = &mut self.transport
        {
            drain_stderr_into(stderr_rx, stderr);
        }
    }

    fn stderr_tail(&mut self) -> String {
        match &mut self.transport {
            Transport::Stdio {
                stderr_rx, stderr, ..
            } => {
                drain_stderr_into(stderr_rx, stderr);
                tail_lines(stderr)
            }
            Transport::Http { .. } => String::new(),
        }
    }
}

/// Pull all pending stderr lines into the rolling buffer, keeping at most the
/// last 80 lines so a chatty server can't grow this unbounded.
fn drain_stderr_into(rx: &Receiver<String>, buffer: &mut Vec<String>) {
    while let Ok(line) = rx.try_recv() {
        buffer.push(line);
    }
    let keep = 80;
    if buffer.len() > keep {
        let drain = buffer.len() - keep;
        buffer.drain(0..drain);
    }
}

fn tail_lines(buffer: &[String]) -> String {
    let start = buffer.len().saturating_sub(20);
    buffer[start..].join("\n")
}

// ── Pure HTTP JSON-RPC helpers (unit-testable, no network) ─────────────────

/// Build a JSON-RPC 2.0 request body. A `None` id produces a notification
/// (no `id` field), which the server answers with no response.
fn build_http_rpc(id: Option<u64>, method: &str, params: Value) -> Value {
    match id {
        Some(id) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }),
        None => json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }),
    }
}

/// Extract the JSON-RPC `result` from a parsed response, or surface its
/// `error`. Mirrors the stdio path's result/error handling exactly.
fn extract_rpc_result(response: &Value, method: &str) -> Result<Value> {
    if let Some(error) = response.get("error") {
        bail!("MCP request '{method}' failed: {error}");
    }
    Ok(response.get("result").cloned().unwrap_or(Value::Null))
}

/// Perform a blocking HTTP JSON-RPC request on a freshly spawned std::thread.
///
/// reqwest::blocking PANICS when constructed inside a tokio runtime, and
/// McpManager runs inside one. Moving the ENTIRE blocking exchange onto a plain
/// std::thread keeps it off the async reactor; the result comes back over an
/// mpsc channel which we wait on with the same ~20s request timeout.
fn request_http(
    url: &str,
    token: Option<&str>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let body = build_http_rpc(Some(id), method, params);
    let method = method.to_string();
    let (tx, rx) = mpsc::channel::<std::result::Result<Value, String>>();
    spawn_http_post(url, token, body, tx);

    match rx.recv_timeout(REQUEST_TIMEOUT) {
        Ok(Ok(value)) => extract_rpc_result(&value, &method),
        Ok(Err(err)) => bail!("MCP HTTP request '{method}' failed: {err}"),
        Err(_) => bail!("MCP HTTP request '{method}' timed out after 20s"),
    }
}

/// Fire-and-forget HTTP notification (no id). The response is ignored.
fn notify_http(url: &str, token: Option<&str>, method: &str, params: Value) -> Result<()> {
    let body = build_http_rpc(None, method, params);
    let (tx, _rx) = mpsc::channel::<std::result::Result<Value, String>>();
    spawn_http_post(url, token, body, tx);
    // Do not wait for or inspect the response: notifications have no reply.
    Ok(())
}

/// Spawn the blocking POST on its own thread. Headers: Authorization (only when
/// a token is present) and Content-Type: application/json. NO Origin header —
/// the browser MCP server rejects any request carrying one.
fn spawn_http_post(
    url: &str,
    token: Option<&str>,
    body: Value,
    tx: mpsc::Sender<std::result::Result<Value, String>>,
) {
    let url = url.to_string();
    let token = token.map(str::to_string);
    thread::spawn(move || {
        let result = (|| -> std::result::Result<Value, String> {
            let client = reqwest::blocking::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|e| e.to_string())?;
            let mut req = client.post(&url).header("Content-Type", "application/json");
            if let Some(token) = &token {
                req = req.header("Authorization", format!("Bearer {token}"));
            }
            let response = req.json(&body).send().map_err(|e| e.to_string())?;
            let status = response.status();
            let text = response.text().map_err(|e| e.to_string())?;
            if text.trim().is_empty() {
                // 202 Accepted (e.g. a notification) carries no body.
                return Ok(Value::Null);
            }
            let value: Value = serde_json::from_str(&text).map_err(|e| {
                format!(
                    "invalid JSON response (HTTP {status}): {e}: {}",
                    truncate(&text, 400)
                )
            })?;
            Ok(value)
        })();
        let _ = tx.send(result);
    });
}

// ── Browser discovery file ─────────────────────────────────────────────────

struct BrowserEndpoint {
    url: String,
    token: Option<String>,
}

/// The discovery file Biscuit Browser writes on MCP start:
/// `<config_dir>/biscuit-browser/mcp.json` (Electron `userData`).
fn browser_discovery_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("could not resolve config directory")?;
    Ok(dir.join("biscuit-browser").join("mcp.json"))
}

/// Poll the discovery file until it appears and parses, or the timeout elapses.
fn poll_browser_discovery(path: &Path, timeout: Duration) -> Result<BrowserEndpoint> {
    let start = Instant::now();
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            if let Some(endpoint) = parse_browser_discovery(&text) {
                return Ok(endpoint);
            }
        }
        if start.elapsed() >= timeout {
            bail!(
                "Biscuit Browser did not publish its MCP endpoint at {} within {}s. \
                 Launch Biscuit Browser manually (npm run dev in biscuit-browser/), then run /install biscuit-browser again.",
                path.display(),
                timeout.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// Parse {url, token, ...} out of the discovery file. Returns None until both a
/// usable url is present (token is optional from our side).
fn parse_browser_discovery(text: &str) -> Option<BrowserEndpoint> {
    let value: Value = serde_json::from_str(text).ok()?;
    let url = value.get("url").and_then(Value::as_str)?.to_string();
    if url.is_empty() {
        return None;
    }
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(BrowserEndpoint { url, token })
}

// ── Plugin registry ────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum PluginKind {
    BiscuitBrowser,
    BiscuitGui,
}

struct Plugin {
    name: &'static str,
    transport: &'static str,
    description: &'static str,
    kind: PluginKind,
}

fn plugin_registry() -> Vec<Plugin> {
    vec![
        Plugin {
            name: "biscuit-browser",
            transport: "http",
            description:
                "Drive Biscuit Browser (open/click/type/read pages) over its local MCP server.",
            kind: PluginKind::BiscuitBrowser,
        },
        Plugin {
            name: "biscuit-gui",
            transport: "stdio",
            description: "Local GUI automation tools via the biscuit-gui-mcp stdio server.",
            kind: PluginKind::BiscuitGui,
        },
    ]
}

/// Resolve the biscuit-gui stdio command: prefer one on PATH, then a release
/// build, then a debug build under the workspace.
fn resolve_biscuit_gui_command(workspace: &Path) -> String {
    if which_on_path("biscuit-gui-mcp") {
        return "biscuit-gui-mcp".to_string();
    }
    let release = workspace.join("target/release/biscuit-gui-mcp");
    if release.exists() {
        return release.to_string_lossy().into_owned();
    }
    let debug = workspace.join("target/debug/biscuit-gui-mcp");
    if debug.exists() {
        return debug.to_string_lossy().into_owned();
    }
    // Fall back to the debug path string so the error (if any) is actionable.
    debug.to_string_lossy().into_owned()
}

/// Whether an executable of the given name is reachable on PATH.
fn which_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

/// Match a slash command by exact prefix with a word boundary, returning the
/// trailing text. `/install foo` matches `/install` (rest = " foo"); `/installer`
/// does not. Mirrors the guard the `/mcp` handler uses.
fn command_rest<'a>(input: &'a str, command: &str) -> Option<&'a str> {
    let rest = input.strip_prefix(command)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest)
    } else {
        None
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

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_deserializes_with_stdio_default() {
        // Old configs predate the transport/url/token fields.
        let json = r#"{"name":"fs","command":"npx server","auto_start":true}"#;
        let config: McpServerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "fs");
        assert_eq!(config.command, "npx server");
        assert!(config.auto_start);
        assert_eq!(config.transport, "stdio");
        assert!(config.url.is_none());
        assert!(config.token.is_none());
    }

    #[test]
    fn old_config_file_deserializes() {
        let json = r#"{"servers":[{"name":"fs","command":"npx server","auto_start":false}]}"#;
        let file: McpConfigFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.servers.len(), 1);
        assert_eq!(file.servers[0].transport, "stdio");
    }

    #[test]
    fn http_config_round_trips() {
        let config = McpServerConfig {
            name: "browser".to_string(),
            command: "http://127.0.0.1:8765/mcp".to_string(),
            auto_start: true,
            transport: "http".to_string(),
            url: Some("http://127.0.0.1:8765/mcp".to_string()),
            token: Some("abc".to_string()),
        };
        let text = serde_json::to_string(&config).unwrap();
        let back: McpServerConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back.transport, "http");
        assert_eq!(back.url.as_deref(), Some("http://127.0.0.1:8765/mcp"));
        assert_eq!(back.token.as_deref(), Some("abc"));
    }

    #[test]
    fn build_http_rpc_request_has_id() {
        let body = build_http_rpc(Some(7), "tools/list", json!({"a": 1}));
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 7);
        assert_eq!(body["method"], "tools/list");
        assert_eq!(body["params"]["a"], 1);
    }

    #[test]
    fn build_http_rpc_notification_has_no_id() {
        let body = build_http_rpc(None, "notifications/initialized", json!({}));
        assert_eq!(body["jsonrpc"], "2.0");
        assert!(body.get("id").is_none());
        assert_eq!(body["method"], "notifications/initialized");
    }

    #[test]
    fn extract_rpc_result_returns_result() {
        let response = json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": []}});
        let result = extract_rpc_result(&response, "tools/list").unwrap();
        assert!(result.get("tools").is_some());
    }

    #[test]
    fn extract_rpc_result_returns_null_when_missing() {
        let response = json!({"jsonrpc": "2.0", "id": 1});
        let result = extract_rpc_result(&response, "ping").unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn extract_rpc_result_surfaces_error() {
        let response =
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "nope"}});
        let err = extract_rpc_result(&response, "tools/list").unwrap_err();
        assert!(err.to_string().contains("failed"));
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn plugin_registry_contains_builtins() {
        let names: Vec<&str> = plugin_registry().iter().map(|p| p.name).collect();
        assert!(names.contains(&"biscuit-browser"));
        assert!(names.contains(&"biscuit-gui"));
    }

    #[test]
    fn biscuit_gui_resolves_to_target_path_when_not_on_path() {
        // With no matching binary on PATH or under a temp workspace, we fall
        // back to the debug target path (a deterministic, actionable default).
        let workspace = std::env::temp_dir().join("biscuits-gui-resolve-test-unlikely");
        let cmd = resolve_biscuit_gui_command(&workspace);
        // Either a PATH hit ("biscuit-gui-mcp") on a dev machine that has it, or
        // the debug fallback path under the workspace.
        assert!(
            cmd == "biscuit-gui-mcp"
                || cmd.ends_with("target/debug/biscuit-gui-mcp")
                || cmd.ends_with("target/release/biscuit-gui-mcp")
        );
    }

    #[test]
    fn parse_browser_discovery_reads_url_and_token() {
        let text = r#"{"url":"http://127.0.0.1:8765/mcp","token":"deadbeef","port":8765}"#;
        let endpoint = parse_browser_discovery(text).unwrap();
        assert_eq!(endpoint.url, "http://127.0.0.1:8765/mcp");
        assert_eq!(endpoint.token.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn parse_browser_discovery_rejects_empty() {
        assert!(parse_browser_discovery("{}").is_none());
        assert!(parse_browser_discovery(r#"{"url":""}"#).is_none());
        assert!(parse_browser_discovery("not json").is_none());
    }

    #[test]
    fn command_rest_respects_word_boundary() {
        assert_eq!(command_rest("/install foo", "/install"), Some(" foo"));
        assert_eq!(command_rest("/install", "/install"), Some(""));
        assert_eq!(command_rest("/installer", "/install"), None);
        assert_eq!(command_rest("/plugins", "/plugin"), None);
        assert_eq!(command_rest("/plugin start x", "/plugin"), Some(" start x"));
    }
}
