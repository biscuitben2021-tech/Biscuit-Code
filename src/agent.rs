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
) -> Result<TurnCapture> {
    let start = Instant::now();
    let mut capture = TurnCapture::default();
    let mut activity = ActivityLog::new(print_final);

    memory.save_turn("user", prompt)?;
    history.push(Msg::new("user", prompt));
    let change_snapshot = memory.change_snapshot()?;

    // Max tool-planning rounds per turn. The old cap of 8 silently truncated
    // any task needing more steps (the model just stopped getting tools and was
    // forced to answer). A turn can still emit multiple tool calls per round.
    const MAX_TOOL_ROUNDS: usize = 25;
    for _ in 0..MAX_TOOL_ROUNDS {
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
        ]);
        // Animated spinner while the model plans (a blocking, non-streaming
        // request); cleared the moment the plan arrives.
        let spinner = print_final.then(|| crate::ui::Spinner::start("thinking…"));
        let plan = llm::complete_history(client, config, &tool_system, history).await?;
        if let Some(spinner) = spinner {
            spinner.stop();
        }
        let parsed = tools::parse_calls(&plan);
        if parsed.calls.is_empty() && parsed.errors.is_empty() {
            break;
        }

        history.push(Msg::new("assistant", plan.clone()));
        memory.save_turn("assistant", &format!("[tool request]\n{plan}"))?;

        for call in parsed.calls {
            // Honor a Ctrl-C that arrived mid-round so we stop before firing the
            // next tool rather than draining the whole batch.
            if perms.stop_requested.load(Ordering::Relaxed) {
                break;
            }
            capture.ordered_tool_calls.push(call.tool.clone());
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
            let result = match if call.tool.eq_ignore_ascii_case("slashcommand") {
                execute_slash_tool(client, config, memory, tools, history, &call).await
            } else {
                tools.execute(client, call).await
            } {
                Ok(result) => result,
                Err(err) => {
                    let msg = format!("tool error: {err}");
                    capture.errors.push(msg.clone());
                    msg
                }
            };
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

    activity.stop_listening();
    let memory_context = memory.system_context(prompt)?;
    let skills_context = tools.skills_context(prompt);
    let mut system_context = join_sections(&[
        &memory_context,
        &tools.system_prompt(),
        &skills_context,
        extra_system_context,
    ]);
    system_context.push_str(
        "\n\nTool planning for this turn is complete. Answer normally now. Do not emit tool_call tags.",
    );
    let input_chars = system_context.len() + history.iter().map(|m| m.text.len()).sum::<usize>();
    if print_final {
        println!("\n{}", crate::ui::assistant_header());
        io::stdout().flush()?;
    }
    let (answer, usage) = if print_final {
        llm::chat(client, config, history, &system_context, input_chars).await?
    } else {
        llm::chat_capture(client, config, history, &system_context, input_chars).await?
    };

    capture.final_message = answer.clone();
    capture.token_usage = usage.snapshot();
    memory.save_turn("assistant", &answer)?;
    memory.after_turn(client, config, prompt, &answer).await?;
    memory.log_changes(&change_snapshot, prompt)?;
    history.push(Msg::new("assistant", answer));
    memory.compact_if_needed(client, config, history).await?;
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
