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

    for _ in 0..8 {
        if perms.stop_requested.load(Ordering::Relaxed) {
            if print_final {
                println!("\n[stopped] agent halted by user");
            }
            break;
        }
        capture.loop_count += 1;
        if print_final {
            println!("\n[working] planning next action...");
        }
        let memory_context = memory.system_context(prompt)?;
        let tool_system = format!(
            "{}\n\n{}\n\n{}",
            memory_context,
            tools.system_prompt(),
            extra_system_context
        );
        let plan = llm::complete_history(client, config, &tool_system, history).await?;
        let calls = tools::parse_calls(&plan)?;
        if calls.is_empty() {
            break;
        }

        history.push(Msg::new("assistant", plan.clone()));
        memory.save_turn("assistant", &format!("[tool request]\n{plan}"))?;

        for call in calls {
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
    }

    activity.stop_listening();
    let memory_context = memory.system_context(prompt)?;
    let system_context = format!(
        "{}\n\n{}\n\n{}\n\nTool planning for this turn is complete. Answer normally now. Do not emit tool_call tags.",
        memory_context,
        tools.system_prompt(),
        extra_system_context
    );
    let input_chars = system_context.len() + history.iter().map(|m| m.text.len()).sum::<usize>();
    if print_final {
        print!("biscuits> ");
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
