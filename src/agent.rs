use crate::{
    activity::ActivityLog,
    llm::{self, Config, Msg, UsageSnapshot},
    memory::MemoryStore,
    permissions::PermissionGuard,
    tools::{self, ToolRuntime},
};
use anyhow::Result;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use std::{
    io::{self, Write},
    sync::atomic::Ordering,
    time::Instant,
};

#[derive(Default, Serialize)]
pub struct TurnCapture {
    pub ordered_tool_calls: Vec<String>,
    pub tool_results: Vec<String>,
    pub final_message: String,
    pub loop_count: usize,
    pub token_usage: UsageSnapshot,
    pub errors: Vec<String>,
    pub runtime_ms: u128,
}

#[allow(clippy::too_many_arguments)]
pub async fn run_turn(
    client: &Client,
    config: &Config,
    memory: &mut MemoryStore,
    tools: &mut ToolRuntime,
    perms: &mut PermissionGuard,
    history: &mut Vec<Msg>,
    prompt: &str,
    extra_system_context: &str,
    print_final: bool,
    depth: usize,
    totals: &mut llm::Totals,
) -> Result<TurnCapture> {
    let start = Instant::now();
    let mut capture = TurnCapture::default();
    let mut activity = ActivityLog::new(print_final);

    // Start the "thinking" spinner the instant the turn begins — before the
    // workspace snapshot / memory prep / first model call — so the user gets
    // immediate feedback and never waits on a blank screen. Each planning round
    // reuses or restarts it; it's always stopped before anything else prints.
    let mut spinner = print_final.then(|| crate::ui::Spinner::start("thinking…"));

    memory.save_turn("user", prompt)?;
    history.push(Msg::new("user", prompt));
    let change_snapshot = memory.change_snapshot()?;

    // Ultracode (max-effort) mode raises the work caps and injects a directive
    // asking the agent to decompose, fan out to sub-agents, and self-verify.
    let ultracode = tools.ultracode();
    let ultracode_ctx = if ultracode {
        tools::ULTRACODE_DIRECTIVE
    } else {
        ""
    };
    let subagent_depth = if ultracode { 2 } else { 1 };

    // Max tool-planning rounds per turn. The old cap of 8 silently truncated
    // any task needing more steps (the model just stopped getting tools and was
    // forced to answer). A turn can still emit multiple tool calls per round.
    let max_tool_rounds = if ultracode { 60 } else { 25 };
    for _ in 0..max_tool_rounds {
        if perms.stop_requested.load(Ordering::Relaxed) {
            if print_final {
                println!("\n{}", crate::ui::stopped("halted by user"));
            }
            break;
        }
        capture.loop_count += 1;
        let memory_context = memory.system_context(prompt)?;
        // Recompute each iteration: a tool call this turn (e.g. /skills disable)
        // can change which skills are enabled, so this must not be cached.
        let skills_context = tools.skills_context(prompt);
        let tool_system = join_sections(&[
            &memory_context,
            &tools.system_prompt(),
            &skills_context,
            extra_system_context,
            ultracode_ctx,
            &tools.focus_context(),
        ]);
        // Keep the spinner running across prep + this round's model call (reuse
        // the one started at turn entry on the first round); cleared the moment
        // the plan arrives.
        if print_final && spinner.is_none() {
            spinner = Some(crate::ui::Spinner::start("thinking…"));
        }
        // Native tool calling (opt-in) when available; on any error fall back to
        // the verified text protocol. Either way `plan` is the assistant text we
        // store in history, keeping persistence text-only.
        let (plan, parsed) = if llm::use_native_tools() {
            match llm::complete_with_tools(
                client,
                config,
                &tool_system,
                history,
                &tools.tool_specs(),
            )
            .await
            {
                Ok((text, raw_calls)) => {
                    let calls: Vec<tools::ToolCall> = raw_calls
                        .into_iter()
                        .map(|(tool, args)| tools::ToolCall { tool, args })
                        .collect();
                    let plan_text = synthesize_plan_text(&text, &calls);
                    (
                        plan_text,
                        tools::ParsedCalls {
                            calls,
                            errors: Vec::new(),
                        },
                    )
                }
                Err(_) => {
                    let text = llm::complete_history(client, config, &tool_system, history).await?;
                    let parsed = tools::parse_calls(&text);
                    (text, parsed)
                }
            }
        } else {
            let text = llm::complete_history(client, config, &tool_system, history).await?;
            let parsed = tools::parse_calls(&text);
            (text, parsed)
        };
        if let Some(spinner) = spinner.take() {
            spinner.stop();
        }
        if parsed.calls.is_empty() && parsed.errors.is_empty() {
            break;
        }

        history.push(Msg::new("assistant", plan.clone()));
        memory.save_turn("assistant", &format!("[tool request]\n{plan}"))?;

        // Pre-execute auto-allowed read-only tools (file reads, web fetches)
        // concurrently so a round that reads several things doesn't pay their
        // latency serially. We only prefetch when no hooks are configured (a
        // pre-tool hook must be able to block a read before it happens) and only
        // for calls that wouldn't prompt for permission. The loop below still
        // does all gating, hooks, activity, and history in order — it just reuses
        // these results instead of re-running the I/O.
        let mut prefetched: std::collections::HashMap<usize, String> =
            std::collections::HashMap::new();
        if !tools.hooks().active() {
            let candidates: Vec<(usize, &tools::ToolCall)> = parsed
                .calls
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    tools::is_readonly_tool(&c.tool) && !c.needs_permission_input(perms)
                })
                .collect();
            if candidates.len() >= 2 {
                let tools_ref: &ToolRuntime = tools;
                let results =
                    futures_util::future::join_all(candidates.into_iter().map(
                        |(i, c)| async move { (i, tools_ref.execute_readonly(client, c).await) },
                    ))
                    .await;
                for (i, result) in results {
                    if let Ok(text) = result {
                        prefetched.insert(i, text);
                    }
                }
            }
        }

        for (idx, call) in parsed.calls.into_iter().enumerate() {
            // Honor a Ctrl-C that arrived mid-round so we stop before firing the
            // next tool rather than draining the whole batch.
            if perms.stop_requested.load(Ordering::Relaxed) {
                break;
            }
            let tool_name = call.tool.clone();
            capture.ordered_tool_calls.push(tool_name.clone());
            if print_final {
                if call.needs_user_input() || call.needs_permission_input(perms) {
                    activity.stop_listening();
                } else {
                    activity.ensure_listening();
                }
                activity.tool_started(call.activity_title(), call.activity_detail());
            }
            // ── Permission gate ──
            if let Err(reason) = perms.gate(&call) {
                let result = format!("permission: {reason}");
                if print_final {
                    activity.tool_finished(&result, &result);
                }
                let msg = format!("<tool_result>\n{}\n</tool_result>", result);
                capture.tool_results.push(result);
                history.push(Msg::new("user", msg.clone()));
                memory.save_turn("tool", &msg)?;
                continue;
            }
            // ── Pre-tool hooks (a non-zero exit vetoes the call) ──
            if tools.hooks().active() {
                let pre = tools.hooks().pre_tool(&tool_name, &call.args);
                if pre.blocked {
                    let result = pre.messages.join("; ");
                    if print_final {
                        activity.tool_finished(&result, &result);
                    }
                    let msg = format!("<tool_result>\n{}\n</tool_result>", result);
                    capture.tool_results.push(result);
                    history.push(Msg::new("user", msg.clone()));
                    memory.save_turn("tool", &msg)?;
                    continue;
                }
            }
            let exec_result: Result<String> = if let Some(pre) = prefetched.remove(&idx) {
                // Already executed concurrently above.
                Ok(pre)
            } else if call.tool.eq_ignore_ascii_case("slashcommand") {
                execute_slash_tool(client, config, memory, tools, history, &call).await
            } else if call.tool.eq_ignore_ascii_case("task") {
                execute_task_tool(
                    client,
                    config,
                    perms.mode,
                    perms.stop_requested.clone(),
                    tools.workspace().to_path_buf(),
                    tools.allow_subagents(),
                    &call,
                    depth,
                    subagent_depth,
                )
                .await
            } else {
                tools.execute(client, call).await
            };
            let result = match exec_result {
                Ok(result) => result,
                Err(err) => {
                    let msg = format!("tool error: {err}");
                    capture.errors.push(msg.clone());
                    msg
                }
            };
            // ── Post-tool hooks (observe-only) ──
            if tools.hooks().active() {
                tools.hooks().post_tool(&tool_name, &result);
            }
            if print_final {
                activity.tool_finished(&tools::brief(&result), &result);
            }
            let msg = format!("<tool_result>\n{}\n</tool_result>", result);
            capture.tool_results.push(result);
            history.push(Msg::new("user", msg.clone()));
            memory.save_turn("tool", &msg)?;
        }

        // Hand malformed tool-call blocks back to the model so it can correct
        // itself, instead of the turn silently dropping them (or, previously,
        // erroring out the whole turn on the first bad block).
        for err in &parsed.errors {
            let msg = format!(
                "<tool_result>\ntool call parse error: {err}\nfix the JSON and re-emit the tool call\n</tool_result>"
            );
            capture.tool_results.push(format!("parse error: {err}"));
            history.push(Msg::new("user", msg.clone()));
            memory.save_turn("tool", &msg)?;
        }
    }

    // Defensive: if the loop exited with the spinner still running (e.g. no tool
    // calls this turn), stop it before any answer output.
    if let Some(spinner) = spinner.take() {
        spinner.stop();
    }

    activity.stop_listening();
    let memory_context = memory.system_context(prompt)?;
    let skills_context = tools.skills_context(prompt);
    let mut system_context = join_sections(&[
        &memory_context,
        &tools.system_prompt(),
        &skills_context,
        extra_system_context,
        ultracode_ctx,
        &tools.focus_context(),
    ]);
    system_context.push_str(
        "\n\nTool planning for this turn is complete. Answer normally now. Do not emit tool_call tags.",
    );
    let input_chars = system_context.len() + history.iter().map(|m| m.text.len()).sum::<usize>();
    // Markdown rendering (default) buffers the answer and prints it formatted;
    // BISCUITS_RENDER=raw restores the live token stream.
    let markdown = print_final && crate::markdown::enabled();
    if print_final {
        println!("\n{}", crate::ui::assistant_header());
        io::stdout().flush()?;
    }
    let (answer, usage) = if print_final && !markdown {
        llm::chat(client, config, history, &system_context, input_chars).await?
    } else {
        // Buffered: no live token printing (markdown mode, or capture mode).
        let spinner = markdown.then(|| crate::ui::Spinner::start("responding…"));
        let result =
            llm::chat_capture(client, config, history, &system_context, input_chars).await?;
        if let Some(spinner) = spinner {
            spinner.stop();
        }
        result
    };
    if markdown {
        println!("{}", crate::markdown::render(&answer));
    }

    capture.final_message = answer.clone();
    capture.token_usage = usage.snapshot();
    // Print the token line NOW — right after the answer and before the (silent)
    // memory extraction / compaction work — so it can't land in the middle of the
    // next message while the user types during that gap.
    if print_final {
        llm::print_usage_snapshot(capture.token_usage, totals);
    }
    memory.save_turn("assistant", &answer)?;
    memory.after_turn(client, config, prompt, &answer).await?;
    memory.log_changes(&change_snapshot, prompt)?;
    history.push(Msg::new("assistant", answer));
    memory.compact_if_needed(client, config, history).await?;
    // Stop hooks fire once the top-level agent finishes its response (not for
    // each nested sub-agent).
    if depth == 0 && tools.hooks().active() {
        tools.hooks().stop();
    }
    capture.runtime_ms = start.elapsed().as_millis();
    Ok(capture)
}

pub async fn run_slash_command(
    client: &Client,
    config: &Config,
    memory: &mut MemoryStore,
    tools: &mut ToolRuntime,
    history: &mut Vec<Msg>,
    input: &str,
) -> Result<Option<String>> {
    let input = input.trim();
    if !input.starts_with('/') {
        return Ok(None);
    }
    if matches!(input, "/exit" | "/quit") {
        return Ok(Some(
            "/exit and /quit are only available at the interactive prompt".into(),
        ));
    }
    if input == "/clear" {
        memory.clear_context(history)?;
        return Ok(Some("context cleared; previous chat is saved".into()));
    }
    // /test: generate / view / update the test contract. Routed here because it
    // needs client + config (for generation) which the tool router doesn't have.
    if let Some(rest) = input.strip_prefix("/test") {
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            let workspace = tools.workspace().to_path_buf();
            let rest = rest.trim();
            let (cmd, args) = match rest.split_once(char::is_whitespace) {
                Some((c, a)) => (c, a.trim()),
                None => (rest, ""),
            };
            let output = match cmd {
                "" | "show" => crate::contract::load(&workspace)
                    .map(|c| c.render())
                    .unwrap_or_else(|| "no test contract yet — run /test plan".into()),
                "plan" => {
                    let contract = crate::contract::generate(client, config, &workspace).await?;
                    contract.save(&workspace)?;
                    format!("test contract generated:\n{}", contract.render())
                }
                "pass" | "fail" => {
                    let (id, note) = match args.split_once(char::is_whitespace) {
                        Some((i, n)) => (i.trim(), n.trim()),
                        None => (args, ""),
                    };
                    if id.is_empty() {
                        format!("usage: /test {cmd} <id> [note]")
                    } else {
                        crate::contract::record_result(&workspace, id, cmd == "pass", note)?;
                        crate::contract::load(&workspace)
                            .map(|c| c.render())
                            .unwrap_or_default()
                    }
                }
                _ => "usage: /test plan | show | pass <id> [note] | fail <id> [note]".into(),
            };
            return Ok(Some(output));
        }
    }
    if let Some(output) = tools.command_output(input)? {
        return Ok(Some(output));
    }
    memory.command_output(input, client, config, history).await
}

async fn execute_slash_tool(
    client: &Client,
    config: &Config,
    memory: &mut MemoryStore,
    tools: &mut ToolRuntime,
    history: &mut Vec<Msg>,
    call: &tools::ToolCall,
) -> Result<String> {
    let command = call
        .args
        .get("command")
        .or_else(|| call.args.get("input"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("SlashCommand requires a command string"))?;
    let output = run_slash_command(client, config, memory, tools, history, command)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not a slash command: {command}"))?;
    Ok(format!("slash command: {command}\n{output}"))
}

/// Run a delegated task in an isolated sub-agent: its own memory session, fresh
/// tool runtime, and inherited permission posture. The sub-agent shares the
/// parent's stop flag so Ctrl-C halts the whole tree. `max_depth` caps nesting
/// (normally 1; ultracode raises it to 2) so fan-out can't recurse without bound.
#[allow(clippy::too_many_arguments)]
async fn execute_task_tool(
    client: &Client,
    config: &Config,
    parent_mode: crate::permissions::Mode,
    parent_stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    workspace: std::path::PathBuf,
    allow: bool,
    call: &tools::ToolCall,
    depth: usize,
    max_depth: usize,
) -> Result<String> {
    if !allow || depth >= max_depth {
        anyhow::bail!("sub-agents cannot spawn further sub-agents");
    }
    let task_prompt = call
        .args
        .get("prompt")
        .or_else(|| call.args.get("task"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Task requires a 'prompt' string"))?;
    let description = call
        .args
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("subtask");

    let sub_root = workspace.join("biscuits/subagents").join(format!(
        "{}-{}",
        now_millis(),
        sanitize_label(description)
    ));
    let mut memory = MemoryStore::open_isolated(workspace.clone(), sub_root)?;
    let mut tools = ToolRuntime::new(workspace.clone())?;
    // The child runs at depth+1; it may itself spawn sub-agents only while that
    // stays under the cap (so ultracode's max_depth=2 allows one more level).
    tools.set_allow_subagents(depth + 1 < max_depth);
    let mut perms = PermissionGuard::open(&workspace);
    perms.set_mode(parent_mode);
    // Share the parent's interrupt flag so one Ctrl-C stops the whole tree.
    perms.stop_requested = parent_stop;
    let mut history = Vec::new();

    let system = format!(
        "<subagent_task>You are a focused sub-agent handling one delegated job: {description}. \
Complete it fully using your tools, then end with a concise report of what you found or changed — \
files touched, key findings, and anything the caller must know. You cannot ask the caller \
questions, so make reasonable assumptions and state them.</subagent_task>"
    );

    // Boxed because run_turn → execute_task_tool → run_turn is a recursive async
    // cycle; the box gives the future a finite size.
    // Sub-agents don't print, so their token totals aren't surfaced — use a
    // throwaway accumulator.
    let mut sub_totals = llm::Totals::default();
    let capture = Box::pin(run_turn(
        client,
        config,
        &mut memory,
        &mut tools,
        &mut perms,
        &mut history,
        task_prompt,
        &system,
        false,
        depth + 1,
        &mut sub_totals,
    ))
    .await?;

    Ok(format!(
        "sub-agent report — {description}:\n{}",
        capture.final_message.trim()
    ))
}

/// Render a native plan (assistant text + structured calls) back into the text
/// representation we store in history. This keeps history text-only — the next
/// request flattens it to plain text, so providers never see an unanswered
/// native tool call — and it stays re-parseable by the text protocol.
fn synthesize_plan_text(text: &str, calls: &[tools::ToolCall]) -> String {
    let mut out = text.trim().to_string();
    for call in calls {
        let json = serde_json::json!({ "tool": call.tool, "args": call.args });
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("<tool_call>{json}</tool_call>"));
    }
    out
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .take(40)
        .collect()
}

/// Join system-prompt sections with blank lines, skipping any that are empty so
/// an absent section (e.g. no selected skills) does not leave a blank gap.
fn join_sections(sections: &[&str]) -> String {
    sections
        .iter()
        .map(|section| section.trim())
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn synthesize_plan_text_roundtrips_through_parse_calls() {
        // A native plan flattened to text must parse back to the same calls, so
        // history stays coherent and re-parseable.
        let calls = vec![
            tools::ToolCall {
                tool: "read".into(),
                args: json!({"path": "a.rs"}),
            },
            tools::ToolCall {
                tool: "bash".into(),
                args: json!({"command": "ls"}),
            },
        ];
        let text = synthesize_plan_text("let me look", &calls);
        let parsed = tools::parse_calls(&text);
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.calls.len(), 2);
        assert_eq!(parsed.calls[0].tool, "read");
        assert_eq!(parsed.calls[0].args["path"], "a.rs");
        assert_eq!(parsed.calls[1].tool, "bash");
    }

    #[test]
    fn synthesize_plan_text_empty_when_nothing_to_say() {
        assert_eq!(synthesize_plan_text("", &[]), "");
    }
}
