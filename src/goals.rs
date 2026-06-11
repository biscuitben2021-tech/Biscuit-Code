use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct GoalStore {
    path: PathBuf,
    state: GoalPlanState,
}

#[derive(Default, Deserialize, Serialize)]
struct GoalPlanState {
    goal: Option<Goal>,
    plan: Option<Plan>,
    updated_at: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct Goal {
    title: String,
    status: GoalStatus,
    requirements: Vec<Requirement>,
    done_criteria: Vec<String>,
    done_check: Option<DoneCheck>,
    updated_at: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GoalStatus {
    #[default]
    InProgress,
    Blocked,
    Done,
}

#[derive(Clone, Deserialize, Serialize)]
struct Requirement {
    id: String,
    text: String,
    status: ItemStatus,
    evidence: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ItemStatus {
    #[default]
    Pending,
    InProgress,
    Done,
    Blocked,
}

#[derive(Clone, Deserialize, Serialize)]
struct DoneCheck {
    requirements_met: bool,
    verified: bool,
    no_errors: bool,
    evidence: Vec<String>,
    checks: Vec<String>,
    notes: String,
    at: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct Plan {
    summary: String,
    steps: Vec<PlanStep>,
    updated_at: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct PlanStep {
    id: String,
    action: String,
    tools: Vec<String>,
    files: Vec<String>,
    status: ItemStatus,
    notes: String,
}

impl GoalStore {
    pub fn open(workspace: &Path) -> Result<Self> {
        let root = workspace.join(".biscuits");
        fs::create_dir_all(&root)?;
        let path = root.join("goal_plan.json");
        let mut clobbered = false;
        let state = if path.exists() {
            let text = fs::read_to_string(&path)?;
            match serde_json::from_str(&text) {
                Ok(state) => state,
                Err(err) => {
                    // Preserve a corrupt/hand-edited file instead of silently
                    // resetting and overwriting it on the next save.
                    let backup = path.with_extension("corrupt.bak");
                    let _ = fs::rename(&path, &backup);
                    eprintln!(
                        "warning: {} could not be parsed ({err}); backed up to {} and starting fresh",
                        path.display(),
                        backup.display()
                    );
                    clobbered = true;
                    GoalPlanState::default()
                }
            }
        } else {
            GoalPlanState::default()
        };
        let store = Self { path, state };
        // Only write on first creation or after backing up a corrupt file; don't
        // rewrite a healthy file just because we opened it.
        if !store.path.exists() || clobbered {
            store.save()?;
        }
        Ok(store)
    }

    pub fn system_prompt(&self) -> String {
        format!(
            r#"<goal_planning_system>
Use the Goal and Plan tools for non-trivial user requests, especially implementation, debugging, or multi-step work.
- Goal is the todo list for user requirements. Create one requirement per concrete user requirement.
- Plan records what you are going to do, which tools you expect to use, and which files you expect to change.
- Keep Goal and Plan updated as work progresses.
- Definition of done: only call Goal mark_done when observations confirm every requirement is marked done, the delivered result fully complies with the user request, verification checks passed, and there are no known errors.
- If any requirement is incomplete, a check fails, or an error remains, keep the goal in_progress or blocked and explain what is left.

Goal tool examples:
<tool_call>{{"tool":"Goal","args":{{"action":"set","title":"Build feature","requirements":["Requirement one","Requirement two"],"done_criteria":["All requirements done","Verification passes","No known errors"]}}}}</tool_call>
<tool_call>{{"tool":"Goal","args":{{"action":"update_requirement","id":"R1","status":"done","evidence":"Implemented and verified"}}}}</tool_call>
<tool_call>{{"tool":"Goal","args":{{"action":"mark_done","requirements_met":true,"verified":true,"no_errors":true,"checks":["cargo test"],"evidence":["All requirements are done"]}}}}</tool_call>

Plan tool example:
<tool_call>{{"tool":"Plan","args":{{"action":"set","summary":"Implement requested feature","steps":[{{"action":"Inspect existing code","tools":["Read","Grep"],"files":["src/main.rs"],"status":"pending"}},{{"action":"Edit Rust modules","tools":["Edit","Bash"],"files":["src/main.rs"],"status":"pending"}}]}}}}</tool_call>

Current goal and plan:
{}
</goal_planning_system>"#,
            self.compact_snapshot()
        )
    }

    pub fn execute_goal(&mut self, args: &Value) -> Result<String> {
        let action = str_arg(args, "action")?.to_lowercase();
        let out = match action.as_str() {
            "set" | "create" | "reset" => self.set_goal(args)?,
            "add" | "add_requirement" => self.add_requirements(args)?,
            "update" | "update_requirement" => self.update_requirement(args)?,
            "status" | "set_status" => self.set_goal_status(args)?,
            "mark_done" | "done" | "complete" => self.mark_done(args)?,
            "list" | "show" => self.render_goal(),
            "clear" => {
                self.state.goal = None;
                self.state.updated_at = now();
                self.save()?;
                "goal cleared".to_string()
            }
            _ => bail!("unknown Goal action: {action}"),
        };
        Ok(out)
    }

    pub fn execute_plan(&mut self, args: &Value) -> Result<String> {
        let action = str_arg(args, "action")?.to_lowercase();
        let out = match action.as_str() {
            "set" | "create" | "reset" => self.set_plan(args)?,
            "add" | "add_step" => self.add_plan_step(args)?,
            "update" | "update_step" => self.update_plan_step(args)?,
            "list" | "show" => self.render_plan(),
            "clear" => {
                self.state.plan = None;
                self.state.updated_at = now();
                self.save()?;
                "plan cleared".to_string()
            }
            _ => bail!("unknown Plan action: {action}"),
        };
        Ok(out)
    }

    pub fn command_output(&mut self, input: &str) -> Result<Option<String>> {
        let output = match input {
            "/goal" | "/goals" | "/todo" | "/todos" => self.render_goal(),
            "/goal json" | "/goals json" => serde_json::to_string_pretty(&self.state.goal)?,
            "/goal clear" | "/goals clear" => {
                self.state.goal = None;
                self.state.updated_at = now();
                self.save()?;
                "goal cleared".to_string()
            }
            "/plan" => self.render_plan(),
            "/plan json" => serde_json::to_string_pretty(&self.state.plan)?,
            "/plan clear" => {
                self.state.plan = None;
                self.state.updated_at = now();
                self.save()?;
                "plan cleared".to_string()
            }
            _ => return Ok(None),
        };
        Ok(Some(output))
    }

    fn set_goal(&mut self, args: &Value) -> Result<String> {
        let title = str_arg(args, "title")?.trim();
        if title.is_empty() {
            bail!("goal title cannot be empty");
        }
        let requirements = requirements_from_value(args.get("requirements"), 1)?;
        if requirements.is_empty() {
            bail!("goal requires at least one requirement");
        }
        let done_criteria = string_vec(args, "done_criteria");
        let t = now();
        self.state.goal = Some(Goal {
            title: title.to_string(),
            status: GoalStatus::InProgress,
            requirements,
            done_criteria,
            done_check: None,
            updated_at: t,
        });
        self.state.updated_at = t;
        self.save()?;
        Ok(self.render_goal())
    }

    fn add_requirements(&mut self, args: &Value) -> Result<String> {
        let goal = self.goal_mut()?;
        let next = goal.requirements.len() + 1;
        let mut requirements = requirements_from_value(args.get("requirements"), next)?;
        if requirements.is_empty() {
            if let Some(text) = args.get("text").and_then(Value::as_str) {
                requirements.push(Requirement {
                    id: format!("R{next}"),
                    text: text.trim().to_string(),
                    status: ItemStatus::Pending,
                    evidence: String::new(),
                });
            }
        }
        if requirements.is_empty() {
            bail!("no requirements provided");
        }
        goal.requirements.append(&mut requirements);
        goal.status = GoalStatus::InProgress;
        goal.done_check = None;
        goal.updated_at = now();
        self.state.updated_at = goal.updated_at;
        self.save()?;
        Ok(self.render_goal())
    }

    fn update_requirement(&mut self, args: &Value) -> Result<String> {
        let id = str_arg(args, "id")?;
        let status = item_status(args.get("status"), ItemStatus::InProgress)?;
        let evidence = args
            .get("evidence")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if status == ItemStatus::Done && evidence.is_empty() {
            bail!("done requirements need evidence");
        }

        let goal = self.goal_mut()?;
        let requirement = goal
            .requirements
            .iter_mut()
            .find(|r| r.id.eq_ignore_ascii_case(id))
            .with_context(|| format!("requirement not found: {id}"))?;
        requirement.status = status;
        if !evidence.is_empty() {
            requirement.evidence = evidence.to_string();
        }
        if goal.status == GoalStatus::Done && status != ItemStatus::Done {
            goal.status = GoalStatus::InProgress;
            goal.done_check = None;
        }
        goal.updated_at = now();
        self.state.updated_at = goal.updated_at;
        self.save()?;
        Ok(self.render_goal())
    }

    fn set_goal_status(&mut self, args: &Value) -> Result<String> {
        let status = str_arg(args, "status")?.to_lowercase();
        if status == "done" {
            bail!("use Goal action mark_done so the done gate can verify requirements and checks");
        }
        let goal = self.goal_mut()?;
        goal.status = match status.as_str() {
            "in_progress" | "progress" | "active" => GoalStatus::InProgress,
            "blocked" => GoalStatus::Blocked,
            _ => bail!("unknown goal status: {status}"),
        };
        goal.updated_at = now();
        self.state.updated_at = goal.updated_at;
        self.save()?;
        Ok(self.render_goal())
    }

    fn mark_done(&mut self, args: &Value) -> Result<String> {
        let goal = self.goal_mut()?;
        if goal.requirements.is_empty() {
            bail!("cannot mark done: goal has no requirements");
        }
        let incomplete = goal
            .requirements
            .iter()
            .filter(|r| r.status != ItemStatus::Done)
            .map(|r| format!("{} {}", r.id, r.text))
            .collect::<Vec<_>>();
        if !incomplete.is_empty() {
            bail!(
                "cannot mark done: incomplete requirements remain: {}",
                incomplete.join("; ")
            );
        }
        if !bool_arg(args, "requirements_met", false) {
            bail!("cannot mark done: requirements_met must be true");
        }
        if !bool_arg(args, "verified", false) {
            bail!("cannot mark done: verified must be true after checks pass");
        }
        if !bool_arg(args, "no_errors", false) {
            bail!("cannot mark done: no_errors must be true");
        }

        let evidence = multi_key_string_vec(args, &["evidence", "deliverables"]);
        if evidence.is_empty() {
            bail!("cannot mark done: provide evidence of completed requirements");
        }
        let checks = multi_key_string_vec(args, &["checks", "verification", "commands"]);
        if checks.is_empty() {
            bail!("cannot mark done: provide verification checks");
        }

        let t = now();
        goal.status = GoalStatus::Done;
        goal.done_check = Some(DoneCheck {
            requirements_met: true,
            verified: true,
            no_errors: true,
            evidence,
            checks,
            notes: args
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            at: t,
        });
        goal.updated_at = t;
        self.state.updated_at = t;
        self.save()?;
        Ok(self.render_goal())
    }

    fn set_plan(&mut self, args: &Value) -> Result<String> {
        let summary = str_arg(args, "summary")?.trim();
        if summary.is_empty() {
            bail!("plan summary cannot be empty");
        }
        let steps = plan_steps_from_value(args.get("steps"), 1)?;
        if steps.is_empty() {
            bail!("plan requires at least one step");
        }
        let t = now();
        self.state.plan = Some(Plan {
            summary: summary.to_string(),
            steps,
            updated_at: t,
        });
        self.state.updated_at = t;
        self.save()?;
        Ok(self.render_plan())
    }

    fn add_plan_step(&mut self, args: &Value) -> Result<String> {
        let plan = self.plan_mut()?;
        let next = plan.steps.len() + 1;
        let action = args
            .get("step")
            .or_else(|| args.get("task"))
            .or_else(|| args.get("description"))
            .and_then(Value::as_str)
            .context("missing step/task/description")?
            .trim();
        if action.is_empty() {
            bail!("plan step cannot be empty");
        }
        plan.steps.push(PlanStep {
            id: format!("P{next}"),
            action: action.to_string(),
            tools: string_vec(args, "tools"),
            files: string_vec(args, "files"),
            status: item_status(args.get("status"), ItemStatus::Pending)?,
            notes: args
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
        plan.updated_at = now();
        self.state.updated_at = plan.updated_at;
        self.save()?;
        Ok(self.render_plan())
    }

    fn update_plan_step(&mut self, args: &Value) -> Result<String> {
        let id = str_arg(args, "id")?;
        let plan = self.plan_mut()?;
        let step = plan
            .steps
            .iter_mut()
            .find(|s| s.id.eq_ignore_ascii_case(id))
            .with_context(|| format!("plan step not found: {id}"))?;
        if let Some(status) = args.get("status") {
            step.status = item_status(Some(status), step.status)?;
        }
        if let Some(notes) = args.get("notes").and_then(Value::as_str) {
            step.notes = notes.to_string();
        }
        if let Some(action) = args
            .get("step")
            .or_else(|| args.get("task"))
            .or_else(|| args.get("description"))
            .and_then(Value::as_str)
        {
            if !action.trim().is_empty() {
                step.action = action.trim().to_string();
            }
        }
        let tools = string_vec(args, "tools");
        if !tools.is_empty() {
            step.tools = tools;
        }
        let files = string_vec(args, "files");
        if !files.is_empty() {
            step.files = files;
        }
        plan.updated_at = now();
        self.state.updated_at = plan.updated_at;
        self.save()?;
        Ok(self.render_plan())
    }

    pub fn render_goal(&self) -> String {
        let Some(goal) = &self.state.goal else {
            return "no active goal".into();
        };
        let mut out = format!("goal: {} ({})\n", goal.title, goal_status_name(goal.status));
        if !goal.done_criteria.is_empty() {
            out.push_str("done criteria:\n");
            for item in &goal.done_criteria {
                out.push_str(&format!("- {item}\n"));
            }
        }
        out.push_str("requirements:\n");
        for req in &goal.requirements {
            out.push_str(&format!(
                "- [{}] {}: {}",
                item_status_name(req.status),
                req.id,
                req.text
            ));
            if !req.evidence.is_empty() {
                out.push_str(&format!(" ({})", req.evidence));
            }
            out.push('\n');
        }
        if let Some(done) = &goal.done_check {
            out.push_str("done check:\n");
            out.push_str(&format!(
                "- requirements_met={} verified={} no_errors={}\n",
                done.requirements_met, done.verified, done.no_errors
            ));
            out.push_str(&format!("- checks: {}\n", done.checks.join("; ")));
            out.push_str(&format!("- evidence: {}\n", done.evidence.join("; ")));
        }
        out.trim_end().to_string()
    }

    pub fn render_plan(&self) -> String {
        let Some(plan) = &self.state.plan else {
            return "no active plan".into();
        };
        let mut out = format!("plan: {}\n", plan.summary);
        for step in &plan.steps {
            out.push_str(&format!(
                "- [{}] {}: {}",
                item_status_name(step.status),
                step.id,
                step.action
            ));
            if !step.tools.is_empty() {
                out.push_str(&format!(" | tools: {}", step.tools.join(", ")));
            }
            if !step.files.is_empty() {
                out.push_str(&format!(" | files: {}", step.files.join(", ")));
            }
            if !step.notes.is_empty() {
                out.push_str(&format!(" | notes: {}", step.notes));
            }
            out.push('\n');
        }
        out.trim_end().to_string()
    }

    fn compact_snapshot(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.render_goal());
        out.push('\n');
        out.push_str(&self.render_plan());
        truncate(&out, 3000)
    }

    fn goal_mut(&mut self) -> Result<&mut Goal> {
        self.state.goal.as_mut().context("no active goal")
    }

    fn plan_mut(&mut self) -> Result<&mut Plan> {
        self.state.plan.as_mut().context("no active plan")
    }

    fn save(&self) -> Result<()> {
        // Atomic write so a crash mid-save can't truncate goal_plan.json.
        let mut json = serde_json::to_string_pretty(&self.state)?;
        json.push('\n');
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json.as_bytes())?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

fn requirements_from_value(value: Option<&Value>, start: usize) -> Result<Vec<Requirement>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().context("requirements must be an array")?;
    let mut out = Vec::new();
    for (offset, item) in items.iter().enumerate() {
        let id = format!("R{}", start + offset);
        let requirement = if let Some(text) = item.as_str() {
            Requirement {
                id,
                text: text.trim().to_string(),
                status: ItemStatus::Pending,
                evidence: String::new(),
            }
        } else {
            let text = item
                .get("text")
                .or_else(|| item.get("requirement"))
                .or_else(|| item.get("task"))
                .and_then(Value::as_str)
                .context("requirement object needs text/requirement/task")?;
            Requirement {
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or(id),
                text: text.trim().to_string(),
                status: item_status(item.get("status"), ItemStatus::Pending)?,
                evidence: item
                    .get("evidence")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }
        };
        if !requirement.text.is_empty() {
            out.push(requirement);
        }
    }
    Ok(out)
}

fn plan_steps_from_value(value: Option<&Value>, start: usize) -> Result<Vec<PlanStep>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value.as_array().context("steps must be an array")?;
    let mut out = Vec::new();
    for (offset, item) in items.iter().enumerate() {
        let id = format!("P{}", start + offset);
        if let Some(action) = item.as_str() {
            if !action.trim().is_empty() {
                out.push(PlanStep {
                    id,
                    action: action.trim().to_string(),
                    tools: Vec::new(),
                    files: Vec::new(),
                    status: ItemStatus::Pending,
                    notes: String::new(),
                });
            }
            continue;
        }
        let action = item
            .get("action")
            .or_else(|| item.get("step"))
            .or_else(|| item.get("task"))
            .and_then(Value::as_str)
            .context("plan step object needs action/step/task")?
            .trim();
        if action.is_empty() {
            continue;
        }
        out.push(PlanStep {
            id: item
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(id),
            action: action.to_string(),
            tools: value_string_vec(item.get("tools")),
            files: value_string_vec(item.get("files")),
            status: item_status(item.get("status"), ItemStatus::Pending)?,
            notes: item
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    Ok(out)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string arg: {key}"))
}

fn bool_arg(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn string_vec(args: &Value, key: &str) -> Vec<String> {
    value_string_vec(args.get(key))
}

fn multi_key_string_vec(args: &Value, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let values = string_vec(args, key);
        if !values.is_empty() {
            return values;
        }
    }
    Vec::new()
}

fn value_string_vec(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        Some(Value::String(text)) if !text.trim().is_empty() => vec![text.trim().to_string()],
        _ => Vec::new(),
    }
}

fn item_status(value: Option<&Value>, default: ItemStatus) -> Result<ItemStatus> {
    let Some(value) = value else {
        return Ok(default);
    };
    let Some(status) = value.as_str() else {
        return Ok(default);
    };
    match status.to_lowercase().as_str() {
        "pending" | "todo" | "not_started" => Ok(ItemStatus::Pending),
        "in_progress" | "progress" | "active" | "doing" => Ok(ItemStatus::InProgress),
        "done" | "complete" | "completed" => Ok(ItemStatus::Done),
        "blocked" => Ok(ItemStatus::Blocked),
        _ => bail!("unknown item status: {status}"),
    }
}

fn goal_status_name(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::InProgress => "in_progress",
        GoalStatus::Blocked => "blocked",
        GoalStatus::Done => "done",
    }
}

fn item_status_name(status: ItemStatus) -> &'static str {
    match status {
        ItemStatus::Pending => "pending",
        ItemStatus::InProgress => "in_progress",
        ItemStatus::Done => "done",
        ItemStatus::Blocked => "blocked",
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn done_requires_all_requirements_and_checks() {
        let dir = temp_workspace("done_requires_all_requirements_and_checks");
        let mut store = GoalStore::open(&dir).unwrap();

        store
            .execute_goal(&json!({
                "action": "set",
                "title": "Build goal system",
                "requirements": ["Track requirements", "Verify completion"]
            }))
            .unwrap();

        let err = store
            .execute_goal(&json!({
                "action": "mark_done",
                "requirements_met": true,
                "verified": true,
                "no_errors": true,
                "checks": ["cargo test"],
                "evidence": ["implementation complete"]
            }))
            .unwrap_err();
        assert!(err.to_string().contains("incomplete requirements"));

        store
            .execute_goal(&json!({
                "action": "update_requirement",
                "id": "R1",
                "status": "done",
                "evidence": "todo list exists"
            }))
            .unwrap();
        store
            .execute_goal(&json!({
                "action": "update_requirement",
                "id": "R2",
                "status": "done",
                "evidence": "done gate rejects incomplete goals"
            }))
            .unwrap();

        let rendered = store
            .execute_goal(&json!({
                "action": "mark_done",
                "requirements_met": true,
                "verified": true,
                "no_errors": true,
                "checks": ["cargo test"],
                "evidence": ["all requirements marked done"]
            }))
            .unwrap();
        assert!(rendered.contains("goal: Build goal system (done)"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn done_requirement_needs_evidence() {
        let dir = temp_workspace("done_requirement_needs_evidence");
        let mut store = GoalStore::open(&dir).unwrap();
        store
            .execute_goal(&json!({
                "action": "set",
                "title": "Build plan",
                "requirements": ["Plan has steps"]
            }))
            .unwrap();
        let err = store
            .execute_goal(&json!({
                "action": "update_requirement",
                "id": "R1",
                "status": "done"
            }))
            .unwrap_err();
        assert!(err.to_string().contains("evidence"));
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
