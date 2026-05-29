use anyhow::{bail, Result};
use serde_json::Value;
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

const MAX_SNAPSHOT_FILES: usize = 2_000;
const MAX_HASH_BYTES: u64 = 1_048_576;

pub struct ObservationSystem {
    workspace: PathBuf,
    last_workspace: WorkspaceSnapshot,
    last_terminal: String,
    last_observation: String,
    web_pages: HashMap<String, WebState>,
}

#[derive(Clone, Default)]
pub struct WorkspaceSnapshot {
    files: HashMap<String, FileState>,
    truncated: bool,
}

#[derive(Clone, Eq, PartialEq)]
struct FileState {
    len: u64,
    modified_nanos: u128,
    hash: Option<u64>,
}

#[derive(Clone)]
struct WebState {
    len: usize,
    hash: u64,
}

#[derive(Default)]
struct WorkspaceDiff {
    created: Vec<String>,
    modified: Vec<String>,
    deleted: Vec<String>,
    truncated: bool,
}

impl ObservationSystem {
    pub fn new(workspace: PathBuf) -> Self {
        let workspace = workspace.canonicalize().unwrap_or(workspace);
        let last_workspace = WorkspaceSnapshot::capture(&workspace).unwrap_or_default();
        Self {
            workspace,
            last_workspace,
            last_terminal: String::new(),
            last_observation: "no observations yet".into(),
            web_pages: HashMap::new(),
        }
    }

    pub fn system_prompt() -> &'static str {
        r#"<observation_system>
After every tool call, the runtime returns an <observation> block containing the action result plus any detected state changes.
Use observations as the source of truth. Do not assume an action worked until terminal output, file changes, webpage state, monitor output, or another observation confirms it.
Use Observe when you need to inspect state directly:
- Observe: {"target":"workspace"} summarizes current workspace files.
- Observe: {"target":"changes"} reports file changes since the previous observation baseline.
- Observe: {"target":"file","path":"relative/file","max_bytes":12000} reads current file state.
- Observe: {"target":"terminal"} shows the latest terminal or monitor output observed by the runtime.
- Observe: {"target":"monitor","id":1} reads a background monitor's output.
- Observe: {"target":"web","url":"https://example.com","max_chars":12000} fetches current webpage text and notes whether it changed since the last fetch.
- Observe: {"target":"screen"} summarizes visible TUI state available to this runtime: latest observation, terminal output, workspace changes, active monitors, and ComputerUse results such as screenshot paths.
</observation_system>"#
    }

    pub fn before_action(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot::capture(&self.workspace).unwrap_or_default()
    }

    pub fn after_action(
        &mut self,
        tool: &str,
        result: String,
        before: WorkspaceSnapshot,
    ) -> String {
        let after = WorkspaceSnapshot::capture(&self.workspace).unwrap_or_default();
        let diff = before.diff(&after);
        self.last_workspace = after;

        let terminal_note = self.terminal_note(tool, &result);
        let webpage_note = self.webpage_note(tool, &result);
        let workspace_changes = render_workspace_diff(&diff);

        let observation = format!(
            "<observation tool=\"{tool}\">\n<action_result>\n{result}\n</action_result>\n<workspace_changes>\n{workspace_changes}\n</workspace_changes>\n<terminal_state>{terminal_note}</terminal_state>\n<webpage_state>{webpage_note}</webpage_state>\n<screen_state>Tool result and state changes above are visible to the agent. Use Observe for follow-up inspection.</screen_state>\n</observation>"
        );
        self.last_observation = truncate(&observation, 8_000);
        observation
    }

    pub fn observe_workspace(&mut self) -> String {
        let current = WorkspaceSnapshot::capture(&self.workspace).unwrap_or_default();
        self.last_workspace = current.clone();
        format!(
            "workspace: {}\n{}",
            self.workspace.display(),
            current.render_summary()
        )
    }

    pub fn observe_changes(&mut self) -> String {
        let current = WorkspaceSnapshot::capture(&self.workspace).unwrap_or_default();
        let diff = self.last_workspace.diff(&current);
        self.last_workspace = current;
        render_workspace_diff(&diff)
    }

    pub fn observe_file(&self, path: &str, max_bytes: usize) -> Result<String> {
        let path = self.resolve(path)?;
        let metadata = fs::metadata(&path)?;
        let bytes = fs::read(&path)?;
        let rel_path = rel(&self.workspace, &path);
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|| "unknown".into());
        let body = if looks_binary(&bytes) {
            "(binary file; content omitted)".to_string()
        } else {
            truncate(&String::from_utf8_lossy(&bytes), max_bytes)
        };
        Ok(format!(
            "file: {rel_path}\nbytes: {}\nmodified_unix: {modified}\ncontent:\n{body}",
            metadata.len()
        ))
    }

    pub fn observe_terminal(&self) -> String {
        if self.last_terminal.trim().is_empty() {
            "no terminal output observed yet".into()
        } else {
            truncate(&self.last_terminal, 12_000)
        }
    }

    pub fn observe_screen(&self, active_monitors: &str) -> String {
        format!(
            "screen_state:\nactive_monitors:\n{}\nlatest_observation:\n{}\nlatest_terminal:\n{}",
            if active_monitors.trim().is_empty() {
                "none"
            } else {
                active_monitors
            },
            truncate(&self.last_observation, 4_000),
            truncate(&self.observe_terminal(), 4_000)
        )
    }

    pub fn observe_webpage(&mut self, url: &str, text: &str) -> String {
        let state = WebState {
            len: text.len(),
            hash: hash_bytes(text.as_bytes()),
        };
        let change = match self.web_pages.insert(url.to_string(), state.clone()) {
            None => "first observation".to_string(),
            Some(previous) if previous.hash == state.hash => {
                format!("unchanged since last observation ({} chars)", state.len)
            }
            Some(previous) => format!(
                "changed since last observation ({} -> {} chars)",
                previous.len, state.len
            ),
        };
        format!(
            "url: {url}\nstate: {change}\ncontent_chars: {}\ncontent:\n{}",
            state.len, text
        )
    }

    pub fn remember_terminal(&mut self, text: String) {
        self.last_terminal = text;
    }

    fn terminal_note(&mut self, tool: &str, result: &str) -> String {
        match tool.to_lowercase().as_str() {
            "bash" | "monitor" | "askuserquestion" | "computeruse" | "computer_use"
            | "computer-use" | "computer use" => {
                self.last_terminal = result.to_string();
                "observed".into()
            }
            _ => "not_applicable".into(),
        }
    }

    fn webpage_note(&mut self, tool: &str, result: &str) -> String {
        match tool.to_lowercase().as_str() {
            "webfetch" => {
                let url = result
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("url: "))
                    .unwrap_or("unknown");
                let state = WebState {
                    len: result.len(),
                    hash: hash_bytes(result.as_bytes()),
                };
                match self.web_pages.insert(url.to_string(), state.clone()) {
                    None => format!("observed {url} (first fetch, {} chars)", state.len),
                    Some(previous) if previous.hash == state.hash => {
                        format!("observed {url} (unchanged, {} chars)", state.len)
                    }
                    Some(previous) => format!(
                        "observed {url} (changed, {} -> {} chars)",
                        previous.len, state.len
                    ),
                }
            }
            "websearch" => "search results observed".into(),
            _ => "not_applicable".into(),
        }
    }

    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let path = self.workspace.join(path).canonicalize()?;
        if !path.starts_with(&self.workspace) {
            bail!("path is outside workspace");
        }
        Ok(path)
    }
}

impl WorkspaceSnapshot {
    fn capture(workspace: &Path) -> Result<Self> {
        let mut snapshot = Self::default();
        collect_snapshot(workspace, workspace, &mut snapshot)?;
        Ok(snapshot)
    }

    fn diff(&self, after: &Self) -> WorkspaceDiff {
        let mut diff = WorkspaceDiff {
            truncated: self.truncated || after.truncated,
            ..Default::default()
        };
        for (path, state) in &after.files {
            match self.files.get(path) {
                None => diff.created.push(path.clone()),
                Some(before) if before != state => diff.modified.push(path.clone()),
                Some(_) => {}
            }
        }
        for path in self.files.keys() {
            if !after.files.contains_key(path) {
                diff.deleted.push(path.clone());
            }
        }
        diff.created.sort();
        diff.modified.sort();
        diff.deleted.sort();
        diff
    }

    fn render_summary(&self) -> String {
        let mut paths = self.files.keys().cloned().collect::<Vec<_>>();
        paths.sort();
        let listed = paths
            .iter()
            .take(40)
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        let suffix = if self.truncated {
            "\n(snapshot truncated)"
        } else {
            ""
        };
        if listed.is_empty() {
            format!("files: 0{suffix}")
        } else {
            format!("files: {}{suffix}\n{}", self.files.len(), listed)
        }
    }
}

pub fn target_arg(args: &Value) -> &str {
    args.get("target")
        .or_else(|| args.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("screen")
}

fn collect_snapshot(root: &Path, dir: &Path, snapshot: &mut WorkspaceSnapshot) -> Result<()> {
    if snapshot.files.len() >= MAX_SNAPSHOT_FILES {
        snapshot.truncated = true;
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if skip_path(root, &path) {
            continue;
        }
        if path.is_dir() {
            collect_snapshot(root, &path, snapshot)?;
        } else if path.is_file() {
            let metadata = fs::metadata(&path)?;
            let rel_path = rel(root, &path);
            let modified_nanos = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            snapshot.files.insert(
                rel_path,
                FileState {
                    len: metadata.len(),
                    modified_nanos,
                    hash: file_hash(&path, metadata.len()),
                },
            );
            if snapshot.files.len() >= MAX_SNAPSHOT_FILES {
                snapshot.truncated = true;
                return Ok(());
            }
        }
    }
    Ok(())
}

fn skip_path(root: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if matches!(
        name,
        ".git" | "target" | "node_modules" | ".next" | "dist" | "build"
    ) {
        return true;
    }
    path.strip_prefix(root)
        .ok()
        // Normalize separators so these checks hold on Windows too.
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map(|rel| {
            rel.starts_with(".biscuits/conversations")
                || rel == ".biscuits/memory_graph.json"
                || is_biscuit_log_path(&rel)
        })
        .unwrap_or(false)
}

fn is_biscuit_log_path(rel: &str) -> bool {
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

fn file_hash(path: &Path, len: u64) -> Option<u64> {
    if len > MAX_HASH_BYTES {
        return None;
    }
    fs::read(path)
        .ok()
        .map(|bytes| hash_bytes(bytes.as_slice()))
}

fn render_workspace_diff(diff: &WorkspaceDiff) -> String {
    if diff.created.is_empty() && diff.modified.is_empty() && diff.deleted.is_empty() {
        return if diff.truncated {
            "none detected (snapshot truncated)".into()
        } else {
            "none detected".into()
        };
    }

    let mut lines = Vec::new();
    push_limited(&mut lines, "created", &diff.created);
    push_limited(&mut lines, "modified", &diff.modified);
    push_limited(&mut lines, "deleted", &diff.deleted);
    if diff.truncated {
        lines.push("(snapshot truncated)".into());
    }
    lines.join("\n")
}

fn push_limited(out: &mut Vec<String>, label: &str, paths: &[String]) {
    for path in paths.iter().take(20) {
        out.push(format!("- {label}: {path}"));
    }
    if paths.len() > 20 {
        out.push(format!("- {label}: ... and {} more", paths.len() - 20));
    }
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(512).any(|b| *b == 0)
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn rel(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        // Normalize to forward slashes for consistent cross-platform matching.
        .replace('\\', "/")
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
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn observes_created_files_after_action() {
        let dir = temp_workspace("observes_created_files_after_action");
        let mut observations = ObservationSystem::new(dir.clone());
        let before = observations.before_action();
        fs::write(dir.join("created.txt"), "hello").unwrap();

        let output = observations.after_action("Write", "wrote created.txt".into(), before);

        assert!(output.contains("<observation tool=\"Write\">"));
        assert!(output.contains("- created: created.txt"));
        assert!(output.contains("wrote created.txt"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn observe_changes_updates_the_baseline() {
        let dir = temp_workspace("observe_changes_updates_the_baseline");
        let mut observations = ObservationSystem::new(dir.clone());
        fs::write(dir.join("one.txt"), "first").unwrap();

        let first = observations.observe_changes();
        let second = observations.observe_changes();

        assert!(first.contains("- created: one.txt"));
        assert_eq!(second, "none detected");
        let _ = fs::remove_dir_all(dir);
    }

    fn temp_workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "biscuits-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
