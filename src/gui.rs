//! macOS GUI testing driver exposed as an MCP stdio server.
//!
//! Unlike `computer_use.rs` (which drives the screen in pixel coordinates), this
//! module "sees" an application through the macOS Accessibility (AX) tree — a
//! structured, human-like description of the UI made of roles, titles and
//! values. Acting on that tree (press a button, set a text field's value) is
//! coordinate-free, so it sidesteps the Retina / point-vs-pixel scaling problem
//! entirely.
//!
//! The driver is published as a small Model Context Protocol (MCP) server over
//! stdio so it can be plugged into Biscuits (or any MCP client) as a dual-mode
//! plugin. The wire framing matches `src/mcp.rs`: newline-delimited JSON, one
//! JSON-RPC object per line.
//!
//! Layout:
//!   * Pure, cross-platform logic (snapshot -> JSON shaping, ref assignment,
//!     JSON-RPC routing via a [`GuiActions`] trait, signature hashing) lives at
//!     module scope and is covered by `#[cfg(test)]` tests. It compiles and runs
//!     everywhere.
//!   * The actual AX FFI lives in `mac` behind `#[cfg(target_os = "macos")]`,
//!     with a `nonmac` stub module for other platforms that returns a clear
//!     "GUI driver is macOS-only" error (mirroring `computer_use.rs`).

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

/// MCP protocol version we advertise; identical to the one the Biscuits MCP
/// client sends (see `src/mcp.rs::CLIENT_PROTOCOL_VERSION`).
pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const SERVER_NAME: &str = "biscuit-gui-mcp";

/// Safety caps for the AX walk so a pathological app (or an accessibility cycle)
/// can't make us spin forever or eat all memory.
const MAX_ELEMENTS: usize = 2000;
const MAX_DEPTH: usize = 40;

// ---------------------------------------------------------------------------
// Pure data model (cross-platform)
// ---------------------------------------------------------------------------

/// A single node distilled from the AX tree. This is platform-agnostic: the
/// macOS layer produces these, and every other piece of logic (JSON shaping,
/// ref assignment, signatures) consumes them, which keeps the testable surface
/// large and the unsafe surface tiny.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AxNode {
    pub role: String,
    pub title: String,
    pub description: String,
    /// String form of AXValue (we only read string values for v1).
    pub value: String,
    pub enabled: bool,
    pub focused: bool,
    pub depth: usize,
    /// Stable ref ("e1", "e2", …) assigned only to actionable elements; `None`
    /// for purely structural containers.
    pub reference: Option<String>,
}

/// Result of walking an application's AX tree once.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub nodes: Vec<AxNode>,
    /// True if the walk hit `MAX_ELEMENTS` and stopped early.
    pub truncated: bool,
}

/// Roles we consider "actionable" and therefore worth assigning a stable ref to.
/// Anything a test driver would plausibly click, type into, toggle or select.
/// Matched case-insensitively against the AX role, with or without the leading
/// "AX" prefix.
const ACTIONABLE_ROLES: &[&str] = &[
    "button",
    "menubutton",
    "popupbutton",
    "menuitem",
    "menubaritem",
    "checkbox",
    "radiobutton",
    "textfield",
    "securetextfield",
    "textarea",
    "searchfield",
    "combobox",
    "slider",
    "incrementor",
    "stepper",
    "link",
    "tab",
    "disclosuretriangle",
    "cell",
    "row",
    "colorwell",
    "segmentedcontrol",
];

/// Normalize an AX role for matching: lowercase and strip a leading "ax".
fn normalize_role(role: &str) -> String {
    let lower = role.trim().to_ascii_lowercase();
    lower.strip_prefix("ax").unwrap_or(&lower).to_string()
}

fn is_actionable(role: &str) -> bool {
    let norm = normalize_role(role);
    ACTIONABLE_ROLES.iter().any(|candidate| *candidate == norm)
}

/// Assign stable references e1..eN to actionable nodes, in document order.
/// Mutates `nodes` in place and returns the number of refs assigned. Idempotent
/// only if refs were cleared first, so callers should pass freshly-walked nodes.
pub fn assign_refs(nodes: &mut [AxNode]) -> usize {
    let mut counter = 0usize;
    for node in nodes.iter_mut() {
        if is_actionable(&node.role) {
            counter += 1;
            node.reference = Some(format!("e{counter}"));
        } else {
            node.reference = None;
        }
    }
    counter
}

/// A short, human-readable label for a node (best-effort, never panics).
fn node_label(node: &AxNode) -> String {
    for candidate in [&node.title, &node.description, &node.value] {
        let trimmed = candidate.trim();
        if !trimmed.is_empty() {
            return one_line(trimmed, 80);
        }
    }
    String::new()
}

/// Shape a snapshot into the JSON object returned by `gui_view`. Pure: no FFI,
/// fully testable. Only nodes with a ref (the actionable ones) are listed, plus
/// summary counts, so the model gets a compact, action-oriented view.
pub fn snapshot_to_json(snapshot: &Snapshot) -> Value {
    let mut elements = Vec::new();
    for node in &snapshot.nodes {
        let Some(reference) = &node.reference else {
            continue;
        };
        let mut entry = json!({
            "ref": reference,
            "role": node.role,
            "enabled": node.enabled,
        });
        let label = node_label(node);
        if !label.is_empty() {
            entry["label"] = json!(label);
        }
        if !node.title.trim().is_empty() {
            entry["title"] = json!(one_line(node.title.trim(), 120));
        }
        if !node.value.trim().is_empty() {
            entry["value"] = json!(one_line(node.value.trim(), 120));
        }
        if node.focused {
            entry["focused"] = json!(true);
        }
        elements.push(entry);
    }

    json!({
        "elements": elements,
        "actionable_count": elements.len(),
        "total_nodes": snapshot.nodes.len(),
        "truncated": snapshot.truncated,
        "signature": snapshot_signature(snapshot),
    })
}

/// A render of the snapshot as plain text, suitable for stuffing into a
/// `tools/call` text content block. Pure / testable.
pub fn snapshot_to_text(snapshot: &Snapshot) -> String {
    let mut out = String::new();
    let mut listed = 0usize;
    for node in &snapshot.nodes {
        let Some(reference) = &node.reference else {
            continue;
        };
        listed += 1;
        let label = node_label(node);
        let role = normalize_role(&node.role);
        let mut line = format!("[{reference}] {role}");
        if !label.is_empty() {
            line.push_str(&format!(" \"{label}\""));
        }
        if !node.enabled {
            line.push_str(" (disabled)");
        }
        if node.focused {
            line.push_str(" (focused)");
        }
        out.push_str(&line);
        out.push('\n');
    }
    if listed == 0 {
        out.push_str("(no actionable elements found)\n");
    }
    if snapshot.truncated {
        out.push_str("… (snapshot truncated at element cap)\n");
    }
    out.push_str(&format!("signature: {}\n", snapshot_signature(snapshot)));
    out
}

// ---------------------------------------------------------------------------
// Signature hashing (cross-platform, testable)
// ---------------------------------------------------------------------------

/// A stable, order-sensitive signature over the roles+titles+values of a
/// snapshot. `gui_click` (and friends) record this before and after acting so
/// they can report whether the screen actually changed — without any pixel
/// comparison.
///
/// Uses FNV-1a (64-bit), a tiny dependency-free hash. We deliberately do NOT use
/// `std::hash::DefaultHasher` because its output is not guaranteed stable across
/// Rust versions or runs (SipHash is randomly keyed in some contexts), which
/// would make signatures useless for before/after comparison.
pub fn snapshot_signature(snapshot: &Snapshot) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            hash ^= b as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    };

    for node in &snapshot.nodes {
        feed(node.role.as_bytes());
        feed(b"\x1f");
        feed(node.title.as_bytes());
        feed(b"\x1f");
        feed(node.value.as_bytes());
        feed(b"\x1e");
    }
    format!("{hash:016x}")
}

// ---------------------------------------------------------------------------
// Actions trait — the seam between pure routing and real (FFI) behaviour
// ---------------------------------------------------------------------------

/// The set of effectful operations the MCP server needs. Production wires this
/// to the macOS AX layer ([`MacDriver`]); tests provide an in-memory fake. This
/// is what makes `dispatch_request` fully testable cross-platform.
pub trait GuiActions {
    /// Launch an application. Exactly one of app/path/bundle_id is expected.
    /// Returns a human-readable status string and stores the resolved pid as the
    /// current target.
    fn launch(&mut self, target: LaunchTarget) -> Result<String>;

    /// Walk the current target's AX tree and return a fresh snapshot.
    fn view(&mut self) -> Result<Snapshot>;

    /// Press / activate the element with the given ref. Returns a status string.
    fn click(&mut self, reference: &str) -> Result<String>;

    /// Set the value of (type text into) the element with the given ref.
    fn type_text(&mut self, reference: &str, text: &str) -> Result<String>;

    /// Send a key combo (e.g. "cmd+s") to the current target / system.
    fn key(&mut self, combo: &str) -> Result<String>;

    /// Capture a screenshot and return a status string (path or data URL).
    fn screenshot(&mut self) -> Result<String>;
}

/// Where to launch an app from. Mirrors the three `open` flavours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchTarget {
    /// Application name, e.g. "Calculator" -> `open -a Calculator`.
    App(String),
    /// Filesystem path, e.g. "/Applications/Foo.app" -> `open <path>`.
    Path(String),
    /// Bundle id, e.g. "com.apple.calculator" -> `open -b <id>`.
    BundleId(String),
}

impl LaunchTarget {
    /// Parse a `gui_launch` arguments object into a [`LaunchTarget`]. Pure /
    /// testable. Precedence: bundle_id > path > app (any one is sufficient).
    pub fn from_args(args: &Value) -> Result<Self> {
        if let Some(id) = args.get("bundle_id").and_then(Value::as_str) {
            let id = id.trim();
            if !id.is_empty() {
                return Ok(LaunchTarget::BundleId(id.to_string()));
            }
        }
        if let Some(path) = args.get("path").and_then(Value::as_str) {
            let path = path.trim();
            if !path.is_empty() {
                return Ok(LaunchTarget::Path(path.to_string()));
            }
        }
        if let Some(app) = args.get("app").and_then(Value::as_str) {
            let app = app.trim();
            if !app.is_empty() {
                return Ok(LaunchTarget::App(app.to_string()));
            }
        }
        bail!("gui_launch requires one of: app, path, bundle_id")
    }
}

// ---------------------------------------------------------------------------
// MCP tool catalogue
// ---------------------------------------------------------------------------

/// The full advertised tool list with JSON-Schema input schemas. Pure /
/// testable; used both by `tools/list` and by the unit tests.
pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "gui_launch",
            "description": "Launch a macOS app and make it the current target. Provide exactly one of app (application name), path (filesystem path to a .app), or bundle_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "Application name, e.g. 'Calculator'." },
                    "path": { "type": "string", "description": "Path to an application bundle, e.g. '/Applications/Foo.app'." },
                    "bundle_id": { "type": "string", "description": "Bundle identifier, e.g. 'com.apple.calculator'." }
                },
                "additionalProperties": false
            }
        },
        {
            "name": "gui_view",
            "description": "Read the current target's Accessibility tree and return a semantic snapshot: a list of actionable elements with stable refs (e1, e2, …), roles, labels, values and enabled/focused state, plus a tree signature.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "gui_click",
            "description": "Press/activate the element with the given ref (coordinate-free AXPress). Reports whether the AX tree signature changed afterwards.",
            "inputSchema": {
                "type": "object",
                "properties": { "ref": { "type": "string", "description": "Element ref from gui_view, e.g. 'e3'." } },
                "required": ["ref"],
                "additionalProperties": false
            }
        },
        {
            "name": "gui_type",
            "description": "Type text into the element with the given ref by setting its AXValue (coordinate-free).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Element ref from gui_view, e.g. 'e2'." },
                    "text": { "type": "string", "description": "Text to set as the element's value." }
                },
                "required": ["ref", "text"],
                "additionalProperties": false
            }
        },
        {
            "name": "gui_key",
            "description": "Send a keyboard combo to the current target, e.g. 'cmd+s', 'enter', 'cmd+shift+n'.",
            "inputSchema": {
                "type": "object",
                "properties": { "combo": { "type": "string", "description": "Key combo, modifiers joined with '+'." } },
                "required": ["combo"],
                "additionalProperties": false
            }
        },
        {
            "name": "gui_screenshot",
            "description": "Capture the screen as a PNG (vision fallback) and return the file path.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "gui_verify",
            "description": "Check whether some expected text is present anywhere in the current target's AX tree (roles/titles/values). Returns pass/fail.",
            "inputSchema": {
                "type": "object",
                "properties": { "expect": { "type": "string", "description": "Substring expected to be visible in the UI." } },
                "required": ["expect"],
                "additionalProperties": false
            }
        },
        {
            "name": "get_test_contract",
            "description": "Read biscuits/test_contract.json from the current working directory and return it. Defines the test cases this driver should execute.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "report_result",
            "description": "Record a test result back into biscuits/test_contract.json: id, pass (bool) and an optional note.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Test case id from get_test_contract." },
                    "pass": { "type": "boolean", "description": "Whether the case passed." },
                    "note": { "type": "string", "description": "Optional explanatory note." }
                },
                "required": ["id", "pass"],
                "additionalProperties": false
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// JSON-RPC routing (pure / testable)
// ---------------------------------------------------------------------------

/// Outcome of routing a single JSON-RPC message.
pub enum Routed {
    /// A response object to write back to the client.
    Reply(Value),
    /// A notification (no `id`) we acknowledge silently — nothing to write.
    Silent,
}

/// Route one parsed JSON-RPC request to a response, using `actions` for any
/// effectful tool. This is the heart of the server and is fully testable: it
/// takes a parsed [`Value`] and a [`GuiActions`] implementation and returns a
/// [`Routed`] without touching stdin/stdout or any FFI.
pub fn dispatch_request<A: GuiActions>(request: &Value, actions: &mut A) -> Routed {
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications have no id and never get a reply.
    let Some(id) = request.get("id").cloned() else {
        return Routed::Silent;
    };

    match method {
        "initialize" => Routed::Reply(success(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )),
        "tools/list" => Routed::Reply(success(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => Routed::Reply(handle_tools_call(id, request, actions)),
        "ping" => Routed::Reply(success(id, json!({}))),
        other => Routed::Reply(error(id, -32601, &format!("method not found: {other}"))),
    }
}

/// Handle a `tools/call` request: extract name + arguments, dispatch to the
/// matching tool, and wrap the outcome in the MCP `content`/`isError` shape.
fn handle_tools_call<A: GuiActions>(id: Value, request: &Value, actions: &mut A) -> Value {
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match call_tool(name, &args, actions) {
        Ok(text) => success(
            id,
            json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false
            }),
        ),
        Err(err) => success(
            id,
            json!({
                "content": [{ "type": "text", "text": format!("error: {err}") }],
                "isError": true
            }),
        ),
    }
}

/// Dispatch a named tool to its implementation. Returns the text payload on
/// success. The contract tools (`get_test_contract`, `report_result`) operate on
/// the filesystem directly and are platform-independent; everything else goes
/// through `actions`.
fn call_tool<A: GuiActions>(name: &str, args: &Value, actions: &mut A) -> Result<String> {
    match name {
        "gui_launch" => {
            let target = LaunchTarget::from_args(args)?;
            actions.launch(target)
        }
        "gui_view" => {
            let snapshot = actions.view()?;
            Ok(snapshot_to_text(&snapshot))
        }
        "gui_click" => {
            let reference = str_arg(args, "ref")?;
            actions.click(reference)
        }
        "gui_type" => {
            let reference = str_arg(args, "ref")?;
            let text = str_arg(args, "text")?;
            actions.type_text(reference, text)
        }
        "gui_key" => {
            let combo = str_arg(args, "combo")?;
            actions.key(combo)
        }
        "gui_screenshot" => actions.screenshot(),
        "gui_verify" => {
            let expect = str_arg(args, "expect")?;
            let snapshot = actions.view()?;
            let found = snapshot_contains(&snapshot, expect);
            Ok(format!(
                "verify: {} (expected: {:?})",
                if found { "PASS" } else { "FAIL" },
                expect
            ))
        }
        "get_test_contract" => get_test_contract(),
        "report_result" => {
            let id = str_arg(args, "id")?;
            let pass = args
                .get("pass")
                .and_then(Value::as_bool)
                .context("report_result requires a boolean 'pass'")?;
            let note = args.get("note").and_then(Value::as_str).unwrap_or("");
            report_result(id, pass, note)
        }
        other => bail!("unknown tool: {other}"),
    }
}

/// Case-insensitive substring search across every node's role/title/value. Pure.
pub fn snapshot_contains(snapshot: &Snapshot, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return true;
    }
    snapshot.nodes.iter().any(|node| {
        node.title.to_lowercase().contains(&needle)
            || node.value.to_lowercase().contains(&needle)
            || node.description.to_lowercase().contains(&needle)
            || node.role.to_lowercase().contains(&needle)
    })
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string arg: {key}"))
}

fn one_line(text: &str, max: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max {
        compact
    } else {
        format!("{}…", compact.chars().take(max).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Test-contract file helpers (cross-platform)
// ---------------------------------------------------------------------------

fn test_contract_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("biscuits")
        .join("test_contract.json")
}

fn get_test_contract() -> Result<String> {
    let path = test_contract_path();
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "no test contract at {}; create biscuits/test_contract.json with your test cases",
            path.display()
        )
    })?;
    // Pretty-print if it parses; otherwise return raw so a malformed file is
    // still visible to the caller.
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => Ok(serde_json::to_string_pretty(&value)?),
        Err(_) => Ok(text),
    }
}

/// Write a result back into the contract. The contract is expected to be a JSON
/// object; results are stored under a top-level "results" object keyed by id, so
/// repeated calls overwrite the prior result for that id. If no contract exists,
/// one is created containing just the results.
fn report_result(id: &str, pass: bool, note: &str) -> Result<String> {
    let path = test_contract_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut root: Value = if path.exists() {
        let text = std::fs::read_to_string(&path)?;
        serde_json::from_str(&text).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }

    let entry = json!({
        "pass": pass,
        "note": note,
        "ts": now_secs(),
    });

    // Ensure `results` exists and is an object, then store the entry.
    if !root["results"].is_object() {
        root["results"] = json!({});
    }
    root["results"][id] = entry;

    let mut file = std::fs::File::create(&path)?;
    file.write_all(serde_json::to_string_pretty(&root)?.as_bytes())?;
    file.write_all(b"\n")?;

    Ok(format!(
        "recorded result for '{id}': {}",
        if pass { "pass" } else { "fail" }
    ))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Server entry point + stdio loop
// ---------------------------------------------------------------------------

/// Run the MCP server: read newline-delimited JSON-RPC from stdin, write
/// newline-delimited JSON-RPC responses to stdout. Matches `src/mcp.rs` framing
/// exactly (one compact JSON object per line, flushed). State (current pid, last
/// view's ref->element map, pre-action signature) lives in the driver across
/// calls.
pub fn serve_stdio() -> Result<()> {
    let mut driver = MacDriver::new();
    serve_with(
        &mut driver,
        std::io::stdin().lock(),
        std::io::stdout().lock(),
    )
}

/// Generic server loop, parameterized over the actions backend and the I/O
/// streams so it can be exercised by an integration test with in-memory pipes.
pub fn serve_with<A, R, W>(actions: &mut A, reader: R, mut writer: W) -> Result<()>
where
    A: GuiActions,
    R: std::io::Read,
    W: Write,
{
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break; // EOF: client closed stdin.
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            // A non-JSON line on stdin isn't a protocol message; skip it rather
            // than tearing down the server.
            Err(_) => continue,
        };

        match dispatch_request(&request, actions) {
            Routed::Reply(response) => {
                let body = serde_json::to_string(&response)?;
                writer.write_all(body.as_bytes())?;
                writer.write_all(b"\n")?;
                writer.flush()?;
            }
            Routed::Silent => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Driver: macOS AX implementation vs non-macOS stub
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub use mac::MacDriver;

#[cfg(not(target_os = "macos"))]
pub use nonmac::MacDriver;

// =====================================================================
// macOS Accessibility driver
// =====================================================================
#[cfg(target_os = "macos")]
mod mac {
    use super::{
        assign_refs, snapshot_signature, AxNode, GuiActions, LaunchTarget, Snapshot, MAX_DEPTH,
        MAX_ELEMENTS,
    };
    use anyhow::{bail, Context, Result};
    use std::ffi::{c_void, CStr, CString};
    use std::path::PathBuf;
    use std::ptr;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    // --- Core Foundation / Application Services opaque pointer types ---
    type CFTypeRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFArrayRef = *const c_void;
    type AXUIElementRef = *const c_void;
    type CFTypeID = usize;
    type CFIndex = isize;
    type Boolean = u8;

    // kCFStringEncodingUTF8
    const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    // AXError success code.
    const AX_SUCCESS: i32 = 0;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> i32;
        fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> i32;
        fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const i8,
            encoding: u32,
        ) -> CFStringRef;
        fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut i8,
            buffer_size: CFIndex,
            encoding: u32,
        ) -> Boolean;
        fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
        fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
        fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
        fn CFStringGetTypeID() -> CFTypeID;
        fn CFArrayGetTypeID() -> CFTypeID;
        fn CFBooleanGetTypeID() -> CFTypeID;
        fn CFBooleanGetValue(boolean: CFTypeRef) -> Boolean;
        fn CFRelease(cf: CFTypeRef);
    }

    /// RAII wrapper that calls `CFRelease` on drop. Used for every Copy/Create'd
    /// CF object so the walk doesn't leak on any path. Construct only from values
    /// you OWN per the Create Rule / Copy Rule (functions named *Create* or
    /// *Copy*).
    pub(super) struct CfOwned(CFTypeRef);
    impl Drop for CfOwned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }
    impl CfOwned {
        fn as_ref(&self) -> CFTypeRef {
            self.0
        }
    }

    /// Create an owned CFString from a Rust &str. Returns None on failure.
    pub(super) fn cfstr(value: &str) -> Option<CfOwned> {
        let c = CString::new(value).ok()?;
        let s =
            unsafe { CFStringCreateWithCString(ptr::null(), c.as_ptr(), KCF_STRING_ENCODING_UTF8) };
        if s.is_null() {
            None
        } else {
            Some(CfOwned(s))
        }
    }

    /// Read a CFString into a Rust String. Does NOT take ownership of `s`.
    fn cfstring_to_string(s: CFStringRef) -> String {
        if s.is_null() {
            return String::new();
        }
        // Length is in UTF-16 units; UTF-8 can need up to 3 bytes per BMP unit
        // and 4 for surrogate pairs, plus the NUL terminator. Allocate
        // generously (4x + 1) to avoid truncation.
        let len = unsafe { CFStringGetLength(s) };
        if len <= 0 {
            return String::new();
        }
        let cap = (len as usize).saturating_mul(4).saturating_add(1);
        let mut buf = vec![0i8; cap];
        let ok = unsafe {
            CFStringGetCString(
                s,
                buf.as_mut_ptr(),
                cap as CFIndex,
                KCF_STRING_ENCODING_UTF8,
            )
        };
        if ok == 0 {
            return String::new();
        }
        let cstr = unsafe { CStr::from_ptr(buf.as_ptr()) };
        cstr.to_string_lossy().into_owned()
    }

    /// Copy a string-valued attribute from an element. Returns "" if absent or
    /// not a string.
    fn copy_string_attr(element: AXUIElementRef, attr: &str) -> String {
        let Some(attr_cf) = cfstr(attr) else {
            return String::new();
        };
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_ref(), &mut value) };
        if err != AX_SUCCESS || value.is_null() {
            return String::new();
        }
        let owned = CfOwned(value);
        // Only treat it as a string if it really is a CFString.
        let is_string = unsafe { CFGetTypeID(owned.as_ref()) == CFStringGetTypeID() };
        if is_string {
            cfstring_to_string(owned.as_ref())
        } else {
            String::new()
        }
    }

    /// Copy a boolean-valued attribute. Returns `default` if absent / not a bool.
    fn copy_bool_attr(element: AXUIElementRef, attr: &str, default: bool) -> bool {
        let Some(attr_cf) = cfstr(attr) else {
            return default;
        };
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_ref(), &mut value) };
        if err != AX_SUCCESS || value.is_null() {
            return default;
        }
        let owned = CfOwned(value);
        let is_bool = unsafe { CFGetTypeID(owned.as_ref()) == CFBooleanGetTypeID() };
        if is_bool {
            unsafe { CFBooleanGetValue(owned.as_ref()) != 0 }
        } else {
            default
        }
    }

    /// Copy the AXChildren array. Returns the owned array wrapper plus the
    /// children pointers (borrowed from the array — valid only while the array
    /// wrapper is alive). We therefore retain each child individually so it stays
    /// valid for the whole walk.
    fn copy_children(element: AXUIElementRef) -> Vec<AXUIElementRef> {
        let Some(attr_cf) = cfstr("AXChildren") else {
            return Vec::new();
        };
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, attr_cf.as_ref(), &mut value) };
        if err != AX_SUCCESS || value.is_null() {
            return Vec::new();
        }
        let owned = CfOwned(value);
        let is_array = unsafe { CFGetTypeID(owned.as_ref()) == CFArrayGetTypeID() };
        if !is_array {
            return Vec::new();
        }
        let count = unsafe { CFArrayGetCount(owned.as_ref()) };
        let mut children = Vec::new();
        let mut i: CFIndex = 0;
        while i < count {
            let child = unsafe { CFArrayGetValueAtIndex(owned.as_ref(), i) };
            if !child.is_null() {
                // CFArrayGetValueAtIndex returns a BORROWED reference whose
                // lifetime is tied to `owned`. Retain it so it survives past the
                // CFRelease of the array (in `owned`'s Drop) and remains valid
                // for the entire walk; the matching CFRelease happens when the
                // snapshot's `retained` vec is dropped.
                unsafe { CFRetain(child) };
                children.push(child as AXUIElementRef);
            }
            i += 1;
        }
        children
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    }

    /// Read one element's attributes into an AxNode (without children).
    fn read_node(element: AXUIElementRef, depth: usize) -> AxNode {
        AxNode {
            role: copy_string_attr(element, "AXRole"),
            title: copy_string_attr(element, "AXTitle"),
            description: copy_string_attr(element, "AXDescription"),
            value: copy_string_attr(element, "AXValue"),
            enabled: copy_bool_attr(element, "AXEnabled", true),
            focused: copy_bool_attr(element, "AXFocused", false),
            depth,
            reference: None,
        }
    }

    /// Depth-first walk of the AX tree starting at `root`. Records up to
    /// MAX_ELEMENTS nodes and stops descending past MAX_DEPTH. Returns the nodes,
    /// a parallel vec of (retained) element pointers, and a truncation flag. The
    /// caller owns the returned element pointers and must CFRelease them.
    fn walk(root: AXUIElementRef) -> (Vec<AxNode>, Vec<AXUIElementRef>, bool) {
        let mut nodes: Vec<AxNode> = Vec::new();
        let mut elements: Vec<AXUIElementRef> = Vec::new();
        let mut truncated = false;

        // Explicit stack of (element, depth) to avoid deep recursion blowing the
        // native stack on pathological trees. The root is retained by the caller
        // (app_element); children are retained inside copy_children. Everything
        // pushed here is owned by us and released by the caller.
        let mut stack: Vec<(AXUIElementRef, usize)> = vec![(root, 0)];
        // Track whether each stack entry is the (caller-owned) root, which we must
        // NOT release here.
        let mut is_root: Vec<bool> = vec![true];

        while let Some((element, depth)) = stack.pop() {
            let entry_is_root = is_root.pop().unwrap_or(false);

            if nodes.len() >= MAX_ELEMENTS {
                truncated = true;
                // Release this element if we own it (i.e. not the root) and won't
                // record it.
                if !entry_is_root {
                    unsafe { CFRelease(element) };
                }
                continue;
            }
            nodes.push(read_node(element, depth));
            elements.push(element); // ownership transfers to `elements`

            if depth >= MAX_DEPTH {
                continue;
            }
            let children = copy_children(element);
            // Push children in reverse so they pop in document order.
            for child in children.into_iter().rev() {
                stack.push((child, depth + 1));
                is_root.push(false);
            }
        }

        (nodes, elements, truncated)
    }

    /// Owns a set of retained AX element pointers and releases them on drop.
    /// The first element (the application element) is NOT owned here — it lives
    /// in `MacDriver::app_element` — so we record how many of the recorded
    /// elements we own (all except the root, which equals the app element).
    struct RetainedElements {
        ptrs: Vec<AXUIElementRef>,
        /// The app element pointer, which we must not release (the driver owns
        /// it). It appears as the first element of a walk.
        app: AXUIElementRef,
    }
    impl Drop for RetainedElements {
        fn drop(&mut self) {
            for &p in &self.ptrs {
                if !p.is_null() && p != self.app {
                    unsafe { CFRelease(p) };
                }
            }
        }
    }

    /// The live macOS driver. Holds the AX application element for the current
    /// target plus the last view's ref->element map so stateful tools work across
    /// calls.
    pub struct MacDriver {
        target_pid: Option<i32>,
        /// Owned AX application element for the current target.
        app_element: Option<AXUIElementRef>,
        /// ref string -> AX element pointer, from the most recent gui_view.
        ref_map: Vec<(String, AXUIElementRef)>,
        /// Owns/releases the element pointers from the last walk (keeps `ref_map`
        /// pointers alive until the next view replaces them).
        retained: Option<RetainedElements>,
        last_signature: Option<String>,
        screenshot_dir: PathBuf,
    }

    impl MacDriver {
        pub fn new() -> Self {
            let screenshot_dir = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("biscuits")
                .join("gui")
                .join("screenshots");
            Self {
                target_pid: None,
                app_element: None,
                ref_map: Vec::new(),
                retained: None,
                last_signature: None,
                screenshot_dir,
            }
        }

        fn ensure_trusted() -> Result<()> {
            let trusted = unsafe { AXIsProcessTrusted() };
            if !trusted {
                bail!(
                    "Accessibility permission is not granted. Open System Settings → \
Privacy & Security → Accessibility and enable the terminal/host app running \
this GUI driver, then try again."
                );
            }
            Ok(())
        }

        fn app(&self) -> Result<AXUIElementRef> {
            self.app_element
                .context("no target app launched yet; call gui_launch first")
        }

        /// Walk the current target and refresh `ref_map` + `last_signature`.
        fn refresh_snapshot(&mut self) -> Result<Snapshot> {
            Self::ensure_trusted()?;
            let app = self.app()?;
            let (mut nodes, elements, truncated) = walk(app);
            assign_refs(&mut nodes);

            // Rebuild ref_map from the freshly-walked nodes/elements. Drop the
            // previous retained set AFTER we've built the new one so we never
            // free pointers that the old ref_map still referenced mid-rebuild.
            let mut new_ref_map = Vec::new();
            for (node, element) in nodes.iter().zip(elements.iter()) {
                if let Some(reference) = &node.reference {
                    new_ref_map.push((reference.clone(), *element));
                }
            }
            self.ref_map = new_ref_map;
            // Replace retained set (drops the old one, releasing its pointers).
            self.retained = Some(RetainedElements {
                ptrs: elements,
                app,
            });

            let snapshot = Snapshot { nodes, truncated };
            self.last_signature = Some(snapshot_signature(&snapshot));
            Ok(snapshot)
        }

        fn element_for_ref(&self, reference: &str) -> Result<AXUIElementRef> {
            self.ref_map
                .iter()
                .find(|(r, _)| r == reference)
                .map(|(_, el)| *el)
                .with_context(|| {
                    format!("unknown element ref '{reference}'; call gui_view to refresh refs")
                })
        }

        fn clear_target(&mut self) {
            // Drop retained children first (they reference into the app subtree),
            // then release the app element.
            self.retained = None;
            self.ref_map.clear();
            if let Some(app) = self.app_element.take() {
                if !app.is_null() {
                    unsafe { CFRelease(app) };
                }
            }
            self.last_signature = None;
            self.target_pid = None;
        }
    }

    impl Default for MacDriver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Drop for MacDriver {
        fn drop(&mut self) {
            self.clear_target();
        }
    }

    impl GuiActions for MacDriver {
        fn launch(&mut self, target: LaunchTarget) -> Result<String> {
            Self::ensure_trusted()?;

            let mut cmd = std::process::Command::new("open");
            let label = match &target {
                LaunchTarget::App(app) => {
                    cmd.arg("-a").arg(app);
                    format!("app {app}")
                }
                LaunchTarget::Path(path) => {
                    cmd.arg(path);
                    format!("path {path}")
                }
                LaunchTarget::BundleId(id) => {
                    cmd.arg("-b").arg(id);
                    format!("bundle {id}")
                }
            };
            let output = cmd
                .output()
                .with_context(|| format!("failed to run `open` for {label}"))?;
            if !output.status.success() {
                bail!(
                    "open failed for {label}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }

            // Give the app a moment to come to the foreground.
            std::thread::sleep(Duration::from_millis(1200));

            // Resolve the frontmost application's pid. We shell out to a tiny
            // AppleScript via osascript: System Events reports the frontmost
            // process and its unix id. This avoids linking AppKit/NSWorkspace and
            // is robust across launch flavours (app name / path / bundle id) since
            // the just-launched app is the one that comes forward.
            let pid = frontmost_pid()
                .context("could not resolve the frontmost application's pid after launch")?;

            // Replace any previous target.
            self.clear_target();

            let app_el = unsafe { AXUIElementCreateApplication(pid) };
            if app_el.is_null() {
                bail!("AXUIElementCreateApplication returned null for pid {pid}");
            }
            self.app_element = Some(app_el);
            self.target_pid = Some(pid);

            Ok(format!(
                "launched {label}; target pid = {pid}. Call gui_view to read the UI."
            ))
        }

        fn view(&mut self) -> Result<Snapshot> {
            self.refresh_snapshot()
        }

        fn click(&mut self, reference: &str) -> Result<String> {
            Self::ensure_trusted()?;
            let before = self.last_signature.clone();
            let element = self.element_for_ref(reference)?;
            let action = cfstr("AXPress").context("failed to create AXPress CFString")?;
            let err = unsafe { AXUIElementPerformAction(element, action.as_ref()) };
            if err != AX_SUCCESS {
                bail!("AXPress on '{reference}' failed (AXError {err})");
            }
            // Let the UI settle, then re-read to compare signatures.
            std::thread::sleep(Duration::from_millis(400));
            let after = self.refresh_snapshot()?;
            let after_sig = snapshot_signature(&after);
            let changed = before.as_deref() != Some(after_sig.as_str());
            Ok(format!(
                "clicked '{reference}'; screen {} (signature {} -> {})",
                if changed { "changed" } else { "unchanged" },
                before.as_deref().unwrap_or("<none>"),
                after_sig
            ))
        }

        fn type_text(&mut self, reference: &str, text: &str) -> Result<String> {
            Self::ensure_trusted()?;
            let element = self.element_for_ref(reference)?;
            let attr = cfstr("AXValue").context("failed to create AXValue CFString")?;
            let value = cfstr(text).context("failed to create value CFString")?;
            let err =
                unsafe { AXUIElementSetAttributeValue(element, attr.as_ref(), value.as_ref()) };
            if err != AX_SUCCESS {
                bail!(
                    "setting AXValue on '{reference}' failed (AXError {err}); the field may not be \
settable via accessibility"
                );
            }
            Ok(format!("set value of '{reference}' to {text:?}"))
        }

        fn key(&mut self, combo: &str) -> Result<String> {
            // Coordinate-free key delivery requires CGEvent posting (covered by
            // computer_use.rs). For v1 the AX driver focuses on AX actions; we
            // surface a clear message rather than silently no-op.
            bail!(
                "gui_key ({combo:?}) is not implemented in the AX driver v1; use AXPress \
(gui_click) / AXValue (gui_type), or the computer_use key tool for raw key events"
            )
        }

        fn screenshot(&mut self) -> Result<String> {
            std::fs::create_dir_all(&self.screenshot_dir)?;
            let path = self
                .screenshot_dir
                .join(format!("gui-{}.png", now_millis()));
            let output = std::process::Command::new("screencapture")
                .arg("-x")
                .arg(&path)
                .output()
                .context("failed to run screencapture")?;
            if !output.status.success() {
                bail!(
                    "screencapture failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            let bytes = std::fs::metadata(&path)
                .with_context(|| {
                    format!(
                        "screenshot not created; grant Screen Recording permission: {}",
                        path.display()
                    )
                })?
                .len();
            if bytes == 0 {
                bail!("screenshot file is empty: {}", path.display());
            }
            Ok(format!(
                "screenshot saved: {} ({bytes} bytes)",
                path.display()
            ))
        }
    }

    /// Resolve the pid of the frontmost application via AppleScript. Returns the
    /// unix process id. Shelling out here is for *pid resolution only* (not for
    /// typing/clicking, which use AX), and avoids linking AppKit.
    fn frontmost_pid() -> Result<i32> {
        let script = "tell application \"System Events\" to get unix id of first process whose frontmost is true";
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .context("failed to run osascript to resolve frontmost pid")?;
        if !output.status.success() {
            bail!(
                "osascript failed resolving frontmost pid: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let text = String::from_utf8_lossy(&output.stdout);
        text.trim()
            .parse::<i32>()
            .with_context(|| format!("unexpected osascript pid output: {:?}", text.trim()))
    }

    fn now_millis() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }
}

// =====================================================================
// Non-macOS stub driver
// =====================================================================
#[cfg(not(target_os = "macos"))]
mod nonmac {
    use super::{GuiActions, LaunchTarget, Snapshot};
    use anyhow::{bail, Result};

    const MSG: &str = "GUI driver is macOS-only (requires the Accessibility API)";

    /// Stub driver for non-macOS builds. Every effectful method returns a clear
    /// macOS-only error, mirroring `computer_use.rs`'s non-macOS stubs.
    pub struct MacDriver;

    impl MacDriver {
        pub fn new() -> Self {
            MacDriver
        }
    }

    impl Default for MacDriver {
        fn default() -> Self {
            Self::new()
        }
    }

    impl GuiActions for MacDriver {
        fn launch(&mut self, _target: LaunchTarget) -> Result<String> {
            bail!(MSG)
        }
        fn view(&mut self) -> Result<Snapshot> {
            bail!(MSG)
        }
        fn click(&mut self, _reference: &str) -> Result<String> {
            bail!(MSG)
        }
        fn type_text(&mut self, _reference: &str, _text: &str) -> Result<String> {
            bail!(MSG)
        }
        fn key(&mut self, _combo: &str) -> Result<String> {
            bail!(MSG)
        }
        fn screenshot(&mut self) -> Result<String> {
            bail!(MSG)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (cross-platform: exercise the pure logic + routing via a fake driver)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, title: &str, value: &str) -> AxNode {
        AxNode {
            role: role.to_string(),
            title: title.to_string(),
            value: value.to_string(),
            enabled: true,
            ..AxNode::default()
        }
    }

    /// In-memory fake driver so dispatch_request is testable everywhere.
    #[derive(Default)]
    struct FakeDriver {
        launched: Option<LaunchTarget>,
        snapshot: Snapshot,
        clicks: Vec<String>,
        typed: Vec<(String, String)>,
    }

    impl GuiActions for FakeDriver {
        fn launch(&mut self, target: LaunchTarget) -> Result<String> {
            self.launched = Some(target.clone());
            Ok(format!("launched {target:?}"))
        }
        fn view(&mut self) -> Result<Snapshot> {
            let mut s = self.snapshot.clone();
            assign_refs(&mut s.nodes);
            Ok(s)
        }
        fn click(&mut self, reference: &str) -> Result<String> {
            self.clicks.push(reference.to_string());
            Ok(format!("clicked {reference}"))
        }
        fn type_text(&mut self, reference: &str, text: &str) -> Result<String> {
            self.typed.push((reference.to_string(), text.to_string()));
            Ok("typed".into())
        }
        fn key(&mut self, combo: &str) -> Result<String> {
            Ok(format!("key {combo}"))
        }
        fn screenshot(&mut self) -> Result<String> {
            Ok("shot".into())
        }
    }

    #[test]
    fn assigns_refs_only_to_actionable_nodes() {
        let mut nodes = vec![
            node("AXWindow", "win", ""),
            node("AXButton", "OK", ""),
            node("AXStaticText", "hello", ""),
            node("AXTextField", "", "abc"),
        ];
        let count = assign_refs(&mut nodes);
        assert_eq!(count, 2);
        assert_eq!(nodes[0].reference, None);
        assert_eq!(nodes[1].reference.as_deref(), Some("e1"));
        assert_eq!(nodes[2].reference, None);
        assert_eq!(nodes[3].reference.as_deref(), Some("e2"));
    }

    #[test]
    fn actionable_matching_is_role_prefix_insensitive() {
        assert!(is_actionable("AXButton"));
        assert!(is_actionable("button"));
        assert!(is_actionable("AXMenuItem"));
        assert!(!is_actionable("AXStaticText"));
        assert!(!is_actionable("AXWindow"));
    }

    #[test]
    fn snapshot_to_json_lists_only_refs() {
        let mut nodes = vec![
            node("AXWindow", "win", ""),
            node("AXButton", "OK", ""),
            node("AXTextField", "Name", "Sam"),
        ];
        assign_refs(&mut nodes);
        let snap = Snapshot {
            nodes,
            truncated: false,
        };
        let value = snapshot_to_json(&snap);
        assert_eq!(value["actionable_count"], 2);
        assert_eq!(value["total_nodes"], 3);
        let elements = value["elements"].as_array().unwrap();
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0]["ref"], "e1");
        assert_eq!(elements[1]["value"], "Sam");
        assert!(value["signature"].is_string());
    }

    #[test]
    fn signature_changes_with_content_and_is_stable() {
        let snap_a = Snapshot {
            nodes: vec![node("AXButton", "OK", "")],
            truncated: false,
        };
        let snap_b = Snapshot {
            nodes: vec![node("AXButton", "Cancel", "")],
            truncated: false,
        };
        let sig_a1 = snapshot_signature(&snap_a);
        let sig_a2 = snapshot_signature(&snap_a);
        let sig_b = snapshot_signature(&snap_b);
        assert_eq!(sig_a1, sig_a2, "same snapshot must hash identically");
        assert_ne!(sig_a1, sig_b, "different content must hash differently");
        assert_eq!(sig_a1.len(), 16);
    }

    #[test]
    fn launch_target_parsing_precedence() {
        let by_app = LaunchTarget::from_args(&json!({ "app": "Calculator" })).unwrap();
        assert_eq!(by_app, LaunchTarget::App("Calculator".into()));

        let by_path = LaunchTarget::from_args(&json!({ "path": "/A.app" })).unwrap();
        assert_eq!(by_path, LaunchTarget::Path("/A.app".into()));

        let by_id = LaunchTarget::from_args(&json!({ "bundle_id": "com.x" })).unwrap();
        assert_eq!(by_id, LaunchTarget::BundleId("com.x".into()));

        // bundle_id wins over the others.
        let mixed =
            LaunchTarget::from_args(&json!({ "app": "A", "path": "/B.app", "bundle_id": "com.c" }))
                .unwrap();
        assert_eq!(mixed, LaunchTarget::BundleId("com.c".into()));

        assert!(LaunchTarget::from_args(&json!({})).is_err());
        assert!(LaunchTarget::from_args(&json!({ "app": "  " })).is_err());
    }

    #[test]
    fn initialize_returns_protocol_and_server_info() {
        let mut driver = FakeDriver::default();
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} });
        let Routed::Reply(resp) = dispatch_request(&req, &mut driver) else {
            panic!("expected reply");
        };
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(resp["result"]["serverInfo"]["name"], SERVER_NAME);
    }

    #[test]
    fn notifications_get_no_reply() {
        let mut driver = FakeDriver::default();
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(matches!(
            dispatch_request(&req, &mut driver),
            Routed::Silent
        ));
    }

    #[test]
    fn tools_list_advertises_all_tools() {
        let mut driver = FakeDriver::default();
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let Routed::Reply(resp) = dispatch_request(&req, &mut driver) else {
            panic!("expected reply");
        };
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        for expected in [
            "gui_launch",
            "gui_view",
            "gui_click",
            "gui_type",
            "gui_key",
            "gui_screenshot",
            "gui_verify",
            "get_test_contract",
            "report_result",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
        // Every tool has an object inputSchema.
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn tools_call_gui_click_dispatches() {
        let mut driver = FakeDriver::default();
        let req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "gui_click", "arguments": { "ref": "e7" } }
        });
        let Routed::Reply(resp) = dispatch_request(&req, &mut driver) else {
            panic!("expected reply");
        };
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(driver.clicks, vec!["e7".to_string()]);
    }

    #[test]
    fn tools_call_missing_arg_is_error_not_panic() {
        let mut driver = FakeDriver::default();
        let req = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": { "name": "gui_click", "arguments": {} }
        });
        let Routed::Reply(resp) = dispatch_request(&req, &mut driver) else {
            panic!("expected reply");
        };
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("missing string arg: ref"));
    }

    #[test]
    fn unknown_method_returns_jsonrpc_error() {
        let mut driver = FakeDriver::default();
        let req = json!({ "jsonrpc": "2.0", "id": 5, "method": "no/such" });
        let Routed::Reply(resp) = dispatch_request(&req, &mut driver) else {
            panic!("expected reply");
        };
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn gui_verify_checks_ax_tree() {
        let mut driver = FakeDriver {
            snapshot: Snapshot {
                nodes: vec![node("AXStaticText", "Total: 42", "")],
                truncated: false,
            },
            ..Default::default()
        };
        let pass = json!({
            "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": { "name": "gui_verify", "arguments": { "expect": "Total: 42" } }
        });
        let Routed::Reply(resp) = dispatch_request(&pass, &mut driver) else {
            panic!("reply");
        };
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("PASS"), "got: {text}");

        let fail = json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "gui_verify", "arguments": { "expect": "nope" } }
        });
        let Routed::Reply(resp) = dispatch_request(&fail, &mut driver) else {
            panic!("reply");
        };
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("FAIL"), "got: {text}");
    }

    #[test]
    fn serve_with_processes_a_full_session() {
        let mut driver = FakeDriver::default();
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n",
            "\n",
            "not json at all\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"gui_key\",\"arguments\":{\"combo\":\"cmd+s\"}}}\n",
        );
        let mut output = Vec::new();
        serve_with(&mut driver, input.as_bytes(), &mut output).unwrap();

        let text = String::from_utf8(output).unwrap();
        // One line per request that has an id (3 of them); the notification, the
        // blank line and the junk line produce no output.
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "got: {text}");
        let first: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["id"], 1);
        assert_eq!(first["result"]["protocolVersion"], PROTOCOL_VERSION);
        let third: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(third["id"], 3);
        // gui_key is unimplemented in the fake? No — the fake implements it, so
        // this should not be an error.
        assert_eq!(third["result"]["isError"], false);
    }

    #[test]
    fn report_result_roundtrips_through_contract_file() {
        // Use a temp cwd so we don't clobber the real biscuits dir.
        let dir = std::env::temp_dir().join(format!("gui_test_{}", now_nanos_test()));
        std::fs::create_dir_all(&dir).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let out = report_result("case-1", true, "looks good").unwrap();
        assert!(out.contains("pass"));

        let contract = get_test_contract().unwrap();
        let parsed: Value = serde_json::from_str(&contract).unwrap();
        assert_eq!(parsed["results"]["case-1"]["pass"], true);
        assert_eq!(parsed["results"]["case-1"]["note"], "looks good");

        // Restore and clean up.
        std::env::set_current_dir(prev).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn now_nanos_test() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    }
}
