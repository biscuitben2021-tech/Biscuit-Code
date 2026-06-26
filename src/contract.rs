//! Test Contract: from what the agent changed plus the project's stated
//! requirements, produce a concrete checklist of things to verify so any AI
//! agent (or human) knows exactly what to test.
//!
//! The flow is: gather context (git diff/status, falling back to the change log
//! for non-git repos) together with the project's knowledge files, ask the model
//! for a strict-JSON list of surfaces and checklist items, then robustly extract
//! that JSON. The list of changed files is populated locally from the diff/status
//! so it never depends on the model getting it right. Every checklist item starts
//! at status "pending"; results are recorded later via `record_result`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// A self-contained checklist of things to test for the current change set.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TestContract {
    /// UNIX seconds (as a plain integer string) when this contract was generated.
    pub generated_at: String,
    /// Files the agent changed, summarized from the diff/status.
    pub changed: Vec<ChangedItem>,
    /// Surfaces touched, e.g. "macos-gui", "web", "cli", "api".
    pub surfaces: Vec<String>,
    /// The concrete things to verify.
    pub checklist: Vec<ChecklistItem>,
}

/// A single changed file plus a short human-readable summary of the change.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ChangedItem {
    pub file: String,
    pub summary: String,
}

/// One checklist entry: a scenario to exercise and the expected outcome.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ChecklistItem {
    pub id: String,
    pub scenario: String,
    pub expect: String,
    /// One of "pending" | "pass" | "fail".
    pub status: String,
    /// Free-form note recorded when a result is logged. Defaults to empty so
    /// legacy JSON written before this field existed still deserializes.
    #[serde(default)]
    pub note: String,
}

/// Valid surface tags. The model is asked to stay within this set; anything else
/// is dropped during parsing.
const VALID_SURFACES: &[&str] = &["macos-gui", "web", "cli", "api"];

const SYSTEM_PROMPT: &str = "\
You are a meticulous test architect. Given a code change and the project's stated \
requirements, you produce a focused checklist of things to test so another agent \
can verify the work is correct and complete. Cover the changed behavior, relevant \
requirements, edge cases, and regressions. Be specific and concrete. \
Return ONLY strict JSON, no prose, no markdown fences.";

impl TestContract {
    /// Atomic temp+rename writes of `biscuits/test_contract.json` (pretty) and a
    /// human-readable `biscuit/test-contract.md` (checkbox list). Parent
    /// directories are created if missing.
    pub fn save(&self, workspace: &Path) -> Result<()> {
        let biscuits_dir = workspace.join("biscuits");
        let biscuit_dir = workspace.join("biscuit");
        fs::create_dir_all(&biscuits_dir)?;
        fs::create_dir_all(&biscuit_dir)?;

        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        atomic_write(&biscuits_dir.join("test_contract.json"), json.as_bytes())?;
        atomic_write(
            &biscuit_dir.join("test-contract.md"),
            self.markdown().as_bytes(),
        )?;
        Ok(())
    }

    /// A concise display: a counts header followed by per-item ✓/✗/• lines.
    pub fn render(&self) -> String {
        let (pass, fail, pending) = self.counts();
        let mut out = format!(
            "test contract ({} item(s): {pass} pass, {fail} fail, {pending} pending)",
            self.checklist.len()
        );
        if !self.surfaces.is_empty() {
            out.push_str(&format!("\nsurfaces: {}", self.surfaces.join(", ")));
        }
        for item in &self.checklist {
            out.push('\n');
            out.push_str(&format!(
                "{} [{}] {} → {}",
                status_mark(&item.status),
                item.id,
                item.scenario,
                item.expect
            ));
            if !item.note.is_empty() {
                out.push_str(&format!(" ({})", item.note));
            }
        }
        out
    }

    /// Pretty markdown with a GitHub-style checkbox per item.
    fn markdown(&self) -> String {
        let (pass, fail, pending) = self.counts();
        let mut out = String::from("# Test Contract\n\n");
        out.push_str(&format!("Generated at: {}\n\n", self.generated_at));
        out.push_str(&format!(
            "Summary: {} item(s) — {pass} pass, {fail} fail, {pending} pending\n\n",
            self.checklist.len()
        ));

        if !self.surfaces.is_empty() {
            out.push_str("## Surfaces\n\n");
            for surface in &self.surfaces {
                out.push_str(&format!("- {surface}\n"));
            }
            out.push('\n');
        }

        out.push_str("## Changed\n\n");
        if self.changed.is_empty() {
            out.push_str("- (no changes detected)\n");
        } else {
            for item in &self.changed {
                if item.summary.is_empty() {
                    out.push_str(&format!("- `{}`\n", item.file));
                } else {
                    out.push_str(&format!("- `{}` — {}\n", item.file, item.summary));
                }
            }
        }
        out.push('\n');

        out.push_str("## Checklist\n\n");
        if self.checklist.is_empty() {
            out.push_str("_(no checklist items)_\n");
        } else {
            for item in &self.checklist {
                let checked = if item.status.eq_ignore_ascii_case("pass") {
                    "x"
                } else {
                    " "
                };
                out.push_str(&format!(
                    "- [{checked}] **{}** — {}\n  - Expect: {}\n",
                    item.id, item.scenario, item.expect
                ));
                if item.status.eq_ignore_ascii_case("fail") {
                    out.push_str("  - Status: FAIL\n");
                }
                if !item.note.is_empty() {
                    out.push_str(&format!("  - Note: {}\n", item.note));
                }
            }
        }
        out
    }

    fn counts(&self) -> (usize, usize, usize) {
        let mut pass = 0;
        let mut fail = 0;
        let mut pending = 0;
        for item in &self.checklist {
            if item.status.eq_ignore_ascii_case("pass") {
                pass += 1;
            } else if item.status.eq_ignore_ascii_case("fail") {
                fail += 1;
            } else {
                pending += 1;
            }
        }
        (pass, fail, pending)
    }
}

/// Load a previously saved contract, or `None` if it is missing or unreadable.
pub fn load(workspace: &Path) -> Option<TestContract> {
    let path = workspace.join("biscuits").join("test_contract.json");
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Load the contract, flip the matching item (case-insensitive id) to
/// "pass"/"fail" with the given note, and save. Errors if no contract exists or
/// the id is unknown.
pub fn record_result(workspace: &Path, id: &str, pass: bool, note: &str) -> Result<()> {
    let mut contract = load(workspace)
        .context("no test contract found; run `/test plan` to generate one first")?;
    let item = contract
        .checklist
        .iter_mut()
        .find(|item| item.id.eq_ignore_ascii_case(id))
        .with_context(|| format!("unknown checklist id: {id}"))?;
    item.status = if pass { "pass" } else { "fail" }.to_string();
    item.note = note.to_string();
    contract.save(workspace)
}

/// Generate a fresh test contract for the current change set.
///
/// Context = `git diff` + `git status --porcelain` (run in `workspace`), or — if
/// this is not a git repo — the tail of `biscuit/logs.md`, plus the contents of
/// BISCUITS.md, biscuit/handoff.md, and biscuits/goal_plan.json when present.
///
/// Only a failed LLM call is an error; malformed or empty model output yields a
/// contract with an empty checklist (the changed[] list is still populated
/// locally from the diff/status).
pub async fn generate(
    client: &reqwest::Client,
    config: &crate::llm::Config,
    workspace: &Path,
) -> Result<TestContract> {
    let (diff_context, changed) = gather_change_context(workspace);
    let knowledge = gather_project_knowledge(workspace);

    let prompt = build_prompt(&diff_context, &knowledge);
    let raw = crate::llm::complete(client, config, SYSTEM_PROMPT, &prompt).await?;

    // Malformed/empty model output is not an error: fall back to an empty
    // checklist (the changed[] list below is still populated locally).
    let (surfaces, checklist) = parse_contract_json(&raw).unwrap_or_default();

    Ok(TestContract {
        generated_at: now_secs_string(),
        changed,
        surfaces,
        checklist,
    })
}

/// Gather the change context and the local list of changed files.
///
/// Prefers git; on any failure (not a repo, git missing, command error) falls
/// back to the tail of `biscuit/logs.md`. The changed-file list is derived from
/// `git status --porcelain` plus the file headers in the diff so it is correct
/// even when git is available but `status` is empty (e.g. only staged changes).
fn gather_change_context(workspace: &Path) -> (String, Vec<ChangedItem>) {
    let diff = run_git(workspace, &["diff"]);
    let status = run_git(workspace, &["status", "--porcelain"]);

    if let (Some(diff), Some(status)) = (&diff, &status) {
        let mut files: BTreeSet<String> = BTreeSet::new();
        files.extend(files_from_status(status));
        files.extend(files_from_diff(diff));
        let changed = files
            .into_iter()
            .map(|file| ChangedItem {
                file,
                summary: String::new(),
            })
            .collect();
        let context = format!(
            "git status --porcelain:\n{}\n\ngit diff:\n{}",
            trim_or_none(status),
            truncate(diff, 16_000)
        );
        return (context, changed);
    }

    // Non-git fallback: the tail of the change log. We don't try to extract a
    // precise file list here (the log format is prose), so changed[] stays empty
    // and the model works from the log text.
    let log = read_log_tail(workspace);
    let context = format!(
        "This is not a git repository (or git is unavailable). \
Recent change log (tail of biscuit/logs.md):\n{}",
        trim_or_none(&log)
    );
    (context, Vec::new())
}

/// Read the contents of the project knowledge files that exist.
fn gather_project_knowledge(workspace: &Path) -> String {
    let mut out = String::new();
    let sources = [
        ("BISCUITS.md", workspace.join("BISCUITS.md")),
        (
            "biscuit/handoff.md",
            workspace.join("biscuit").join("handoff.md"),
        ),
        (
            "biscuits/goal_plan.json",
            workspace.join("biscuits").join("goal_plan.json"),
        ),
    ];
    for (label, path) in sources {
        if let Ok(text) = fs::read_to_string(&path) {
            let text = text.trim();
            if !text.is_empty() {
                out.push_str(&format!(
                    "<{label}>\n{}\n</{label}>\n\n",
                    truncate(text, 8_000)
                ));
            }
        }
    }
    out
}

fn build_prompt(diff_context: &str, knowledge: &str) -> String {
    format!(
        "Produce a test checklist for the change below.\n\n\
<change_context>\n{}\n</change_context>\n\n\
<project_knowledge>\n{}\n</project_knowledge>\n\n\
Return STRICT JSON in exactly this shape and nothing else:\n\
{{\"surfaces\":[\"...\"],\"checklist\":[{{\"id\":\"T1\",\"scenario\":\"...\",\"expect\":\"...\"}}]}}\n\
Rules:\n\
- surfaces must be chosen only from: {}.\n\
- Each checklist item needs a short stable id (e.g. T1, T2), a concrete scenario to exercise, and the expected result.\n\
- Cover changed behavior, relevant project requirements, edge cases, and likely regressions.\n\
- Do not include a status field; do not wrap the JSON in markdown fences.",
        diff_context,
        if knowledge.trim().is_empty() {
            "(no project knowledge files found)"
        } else {
            knowledge.trim()
        },
        VALID_SURFACES.join(", ")
    )
}

/// Robustly extract the surfaces + checklist from a model response.
///
/// Pure (no I/O): strips markdown fences, locates the first string-aware
/// balanced `{...}` object, parses it, keeps only valid surfaces, and forces
/// every checklist item to status="pending" with an empty note. Returns `None`
/// when no parseable object is found.
fn parse_contract_json(raw: &str) -> Option<(Vec<String>, Vec<ChecklistItem>)> {
    let stripped = strip_fences(raw);
    let candidate = first_balanced_object(stripped)?;
    let value: Value = serde_json::from_str(candidate).ok()?;

    let surfaces = value
        .get("surfaces")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| VALID_SURFACES.contains(&s.to_lowercase().as_str()))
                .map(|s| s.to_lowercase())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut checklist = Vec::new();
    if let Some(items) = value.get("checklist").and_then(Value::as_array) {
        for (offset, item) in items.iter().enumerate() {
            let scenario = item
                .get("scenario")
                .or_else(|| item.get("test"))
                .or_else(|| item.get("description"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            let expect = item
                .get("expect")
                .or_else(|| item.get("expected"))
                .or_else(|| item.get("expectation"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            if scenario.is_empty() && expect.is_empty() {
                continue;
            }
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("T{}", offset + 1));
            checklist.push(ChecklistItem {
                id,
                scenario,
                expect,
                status: "pending".to_string(),
                note: String::new(),
            });
        }
    }

    Some((surfaces, checklist))
}

/// Strip a single leading/trailing markdown code fence (```json … ```), if any.
fn strip_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop the rest of the opening fence line (e.g. ```json) and the closing
    // fence, leaving the body.
    let body = match rest.find('\n') {
        Some(pos) => &rest[pos + 1..],
        None => rest,
    };
    match body.rfind("```") {
        Some(pos) => body[..pos].trim(),
        None => body.trim(),
    }
}

/// Find the first string-aware balanced `{...}` substring. Braces inside JSON
/// string literals (and escaped quotes) are ignored, so `{"k":"a}b"}` is matched
/// in full rather than truncated at the brace inside the string.
fn first_balanced_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Run a git subcommand in `workspace`, returning its stdout on success or
/// `None` if git is unavailable, the directory is not a repo, or it errored.
fn run_git(workspace: &Path, args: &[&str]) -> Option<String> {
    let quoted: Vec<String> = args.iter().map(|a| crate::shell::quote(a)).collect();
    let command_line = format!("git {}", quoted.join(" "));
    let output = crate::shell::command(&command_line)
        .current_dir(workspace)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Parse changed paths out of `git status --porcelain` lines. Handles renames
/// ("R old -> new") by taking the destination path.
fn files_from_status(status: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        // Porcelain v1: two status chars, a space, then the path.
        let rest = line[3..].trim();
        let path = match rest.split_once(" -> ") {
            Some((_, dest)) => dest,
            None => rest,
        };
        let path = path.trim().trim_matches('"');
        if !path.is_empty() {
            out.push(path.to_string());
        }
    }
    out
}

/// Parse changed paths out of unified `git diff` output via its `+++ b/<path>`
/// headers (and `--- a/<path>` for deletions where there is no `+++`).
fn files_from_diff(diff: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in diff.lines() {
        let path = if let Some(rest) = line.strip_prefix("+++ ") {
            normalize_diff_path(rest)
        } else if let Some(rest) = line.strip_prefix("--- ") {
            normalize_diff_path(rest)
        } else {
            None
        };
        if let Some(path) = path {
            out.push(path);
        }
    }
    out
}

/// Normalize a diff header path: strip the leading `a/` or `b/`, ignore
/// `/dev/null` (added/deleted sentinel), and drop a trailing tab+timestamp.
fn normalize_diff_path(rest: &str) -> Option<String> {
    let raw = rest.split('\t').next().unwrap_or(rest).trim();
    if raw.is_empty() || raw == "/dev/null" {
        return None;
    }
    let path = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw);
    Some(path.to_string())
}

/// Read roughly the last 8 KB of `biscuit/logs.md` for the non-git fallback.
fn read_log_tail(workspace: &Path) -> String {
    let path = workspace.join("biscuit").join("logs.md");
    let Ok(text) = fs::read_to_string(path) else {
        return String::new();
    };
    let max = 8_000;
    if text.len() <= max {
        text
    } else {
        // Take the tail on a char boundary so we never split a multi-byte char.
        let start = text.len() - max;
        let start = (start..text.len())
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(text.len());
        text[start..].to_string()
    }
}

fn status_mark(status: &str) -> char {
    if status.eq_ignore_ascii_case("pass") {
        '✓'
    } else if status.eq_ignore_ascii_case("fail") {
        '✗'
    } else {
        '•'
    }
}

fn trim_or_none(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "(none)".to_string()
    } else {
        trimmed.to_string()
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

/// Atomic write: write to a sibling temp file, then rename over the target so a
/// crash mid-write can never leave a truncated file (mirrors goals.rs).
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn now_secs_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_contract() -> TestContract {
        TestContract {
            generated_at: "1700000000".to_string(),
            changed: vec![ChangedItem {
                file: "src/contract.rs".to_string(),
                summary: "added test contract module".to_string(),
            }],
            surfaces: vec!["cli".to_string()],
            checklist: vec![
                ChecklistItem {
                    id: "T1".to_string(),
                    scenario: "generate a contract".to_string(),
                    expect: "checklist is produced".to_string(),
                    status: "pending".to_string(),
                    note: String::new(),
                },
                ChecklistItem {
                    id: "T2".to_string(),
                    scenario: "save then load".to_string(),
                    expect: "round-trips".to_string(),
                    status: "pass".to_string(),
                    note: "verified".to_string(),
                },
            ],
        }
    }

    #[test]
    fn serde_round_trip() {
        let contract = sample_contract();
        let json = serde_json::to_string(&contract).unwrap();
        let back: TestContract = serde_json::from_str(&json).unwrap();
        assert_eq!(back.generated_at, contract.generated_at);
        assert_eq!(back.changed.len(), 1);
        assert_eq!(back.changed[0].file, "src/contract.rs");
        assert_eq!(back.surfaces, vec!["cli".to_string()]);
        assert_eq!(back.checklist.len(), 2);
        assert_eq!(back.checklist[1].status, "pass");
        assert_eq!(back.checklist[1].note, "verified");
    }

    #[test]
    fn legacy_checklist_without_note_still_deserializes() {
        // JSON written before `note` existed must still load (note defaults to "").
        let legacy = r#"{
            "id": "T1",
            "scenario": "do a thing",
            "expect": "it works",
            "status": "pending"
        }"#;
        let item: ChecklistItem = serde_json::from_str(legacy).unwrap();
        assert_eq!(item.id, "T1");
        assert_eq!(item.note, "");
    }

    #[test]
    fn parse_clean_json() {
        let raw =
            r#"{"surfaces":["cli","api"],"checklist":[{"id":"A","scenario":"s","expect":"e"}]}"#;
        let (surfaces, checklist) = parse_contract_json(raw).unwrap();
        assert_eq!(surfaces, vec!["cli".to_string(), "api".to_string()]);
        assert_eq!(checklist.len(), 1);
        assert_eq!(checklist[0].id, "A");
        assert_eq!(checklist[0].scenario, "s");
        assert_eq!(checklist[0].expect, "e");
        // status is always forced to pending regardless of model output.
        assert_eq!(checklist[0].status, "pending");
        assert_eq!(checklist[0].note, "");
    }

    #[test]
    fn parse_fenced_json() {
        let raw = "```json\n{\"surfaces\":[\"web\"],\"checklist\":[{\"scenario\":\"open page\",\"expect\":\"renders\"}]}\n```";
        let (surfaces, checklist) = parse_contract_json(raw).unwrap();
        assert_eq!(surfaces, vec!["web".to_string()]);
        assert_eq!(checklist.len(), 1);
        // No id supplied → synthesized stable id.
        assert_eq!(checklist[0].id, "T1");
    }

    #[test]
    fn parse_noisy_json() {
        let raw = "Sure! Here is the contract you asked for:\n\
            {\"surfaces\":[\"macos-gui\"],\"checklist\":[{\"id\":\"X1\",\"scenario\":\"click button\",\"expect\":\"action fires\"}]}\n\
            Let me know if you need anything else.";
        let (surfaces, checklist) = parse_contract_json(raw).unwrap();
        assert_eq!(surfaces, vec!["macos-gui".to_string()]);
        assert_eq!(checklist.len(), 1);
        assert_eq!(checklist[0].id, "X1");
    }

    #[test]
    fn parse_brace_in_string() {
        // A `}` inside a JSON string must not prematurely close the object.
        let raw = r#"{"surfaces":[],"checklist":[{"id":"B","scenario":"handle literal } brace","expect":"no truncation {ok}"}]}"#;
        let (surfaces, checklist) = parse_contract_json(raw).unwrap();
        assert!(surfaces.is_empty());
        assert_eq!(checklist.len(), 1);
        assert_eq!(checklist[0].scenario, "handle literal } brace");
        assert_eq!(checklist[0].expect, "no truncation {ok}");
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert!(parse_contract_json("not json at all").is_none());
        assert!(parse_contract_json("").is_none());
        assert!(parse_contract_json("{ unterminated").is_none());
    }

    #[test]
    fn parse_drops_invalid_surfaces() {
        let raw = r#"{"surfaces":["cli","quantum","WEB"],"checklist":[]}"#;
        let (surfaces, checklist) = parse_contract_json(raw).unwrap();
        // "quantum" dropped; "WEB" normalized to "web".
        assert_eq!(surfaces, vec!["cli".to_string(), "web".to_string()]);
        assert!(checklist.is_empty());
    }

    #[test]
    fn render_counts_and_marks() {
        let contract = sample_contract();
        let rendered = contract.render();
        assert!(rendered.contains("2 item(s)"));
        assert!(rendered.contains("1 pass"));
        assert!(rendered.contains("1 fail") || rendered.contains("0 fail"));
        assert!(rendered.contains("1 pending"));
        // T2 is pass → ✓ marker; T1 is pending → • marker.
        assert!(rendered.contains('✓'));
        assert!(rendered.contains('•'));
        assert!(rendered.contains("[T1]"));
        assert!(rendered.contains("[T2]"));
    }

    #[test]
    fn render_marks_failures() {
        let mut contract = sample_contract();
        contract.checklist[0].status = "fail".to_string();
        let rendered = contract.render();
        assert!(rendered.contains('✗'));
    }

    #[test]
    fn save_then_load_round_trip_and_markdown_has_checkboxes() {
        let dir = temp_workspace("save_load");
        let contract = sample_contract();
        contract.save(&dir).unwrap();

        let loaded = load(&dir).expect("should load");
        assert_eq!(loaded.checklist.len(), 2);
        assert_eq!(loaded.generated_at, "1700000000");

        let md = fs::read_to_string(dir.join("biscuit").join("test-contract.md")).unwrap();
        assert!(
            md.contains("- [ ]"),
            "markdown should contain an unchecked box"
        );
        assert!(
            md.contains("- [x]"),
            "markdown should contain a checked box for the passing item"
        );
        assert!(md.contains("# Test Contract"));

        // JSON file exists and is pretty-printed (multi-line).
        let json = fs::read_to_string(dir.join("biscuits").join("test_contract.json")).unwrap();
        assert!(json.contains('\n'));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_missing_returns_none() {
        let dir = temp_workspace("load_missing");
        assert!(load(&dir).is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn record_result_flips_status() {
        let dir = temp_workspace("record_result");
        sample_contract().save(&dir).unwrap();

        record_result(&dir, "T1", true, "looks good").unwrap();
        let loaded = load(&dir).unwrap();
        let t1 = loaded.checklist.iter().find(|i| i.id == "T1").unwrap();
        assert_eq!(t1.status, "pass");
        assert_eq!(t1.note, "looks good");

        // Case-insensitive id match.
        record_result(&dir, "t1", false, "regressed").unwrap();
        let loaded = load(&dir).unwrap();
        let t1 = loaded.checklist.iter().find(|i| i.id == "T1").unwrap();
        assert_eq!(t1.status, "fail");
        assert_eq!(t1.note, "regressed");

        // Unknown id errors.
        let err = record_result(&dir, "nope", true, "").unwrap_err();
        assert!(err.to_string().contains("unknown checklist id"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn record_result_without_contract_errors() {
        let dir = temp_workspace("record_no_contract");
        let err = record_result(&dir, "T1", true, "").unwrap_err();
        assert!(err.to_string().contains("no test contract"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn files_from_status_handles_renames_and_paths() {
        let status = " M src/contract.rs\n?? new_file.rs\nR  old.rs -> renamed.rs\n";
        let files = files_from_status(status);
        assert!(files.contains(&"src/contract.rs".to_string()));
        assert!(files.contains(&"new_file.rs".to_string()));
        assert!(files.contains(&"renamed.rs".to_string()));
        assert!(!files.contains(&"old.rs".to_string()));
    }

    #[test]
    fn files_from_diff_strips_prefixes_and_dev_null() {
        let diff = "diff --git a/src/x.rs b/src/x.rs\n--- a/src/x.rs\n+++ b/src/x.rs\n@@\n+line\n\
                    diff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n";
        let files = files_from_diff(diff);
        assert!(files.contains(&"src/x.rs".to_string()));
        assert!(files.contains(&"gone.rs".to_string()));
        assert!(!files.iter().any(|f| f.contains("dev/null")));
        assert!(!files
            .iter()
            .any(|f| f.starts_with("a/") || f.starts_with("b/")));
    }

    #[test]
    fn strip_fences_handles_plain_and_fenced() {
        assert_eq!(strip_fences("  {\"a\":1}  "), "{\"a\":1}");
        assert_eq!(strip_fences("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fences("```\n{\"a\":1}\n```"), "{\"a\":1}");
    }

    #[test]
    fn first_balanced_object_respects_strings() {
        assert_eq!(first_balanced_object("xx{}yy"), Some("{}"));
        assert_eq!(first_balanced_object(r#"{"k":"}"}"#), Some(r#"{"k":"}"}"#));
        assert_eq!(first_balanced_object("no braces"), None);
        assert_eq!(first_balanced_object("{ unbalanced"), None);
    }

    fn temp_workspace(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "biscuits-contract-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
