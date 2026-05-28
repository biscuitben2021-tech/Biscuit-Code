mod activity;
mod agent;
mod computer_use;
mod evals;
mod goals;
mod llm;
mod mcp;
mod memory;
mod observations;
mod permissions;
mod shell;
mod test_harness;
mod tools;

use anyhow::Result;
use llm::Msg;
use memory::MemoryStore;
use permissions::PermissionGuard;
use reqwest::Client;
use std::{
    env,
    io::{self, Write},
};

pub async fn run() -> Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if let Some(cmd) = args.first().map(String::as_str) {
        match cmd {
            "eval" => {
                let code = evals::handle_cli(&args[1..], &env::current_dir()?).await?;
                std::process::exit(code);
            }
            "harness" | "test-harness" => {
                let code = test_harness::handle_cli(&args[1..], &env::current_dir()?)?;
                std::process::exit(code);
            }
            _ => {}
        }
    }

    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("run: biscuits");
        println!("alias: biscuit");
        println!("eval: biscuits eval --smoke");
        println!("harness: biscuits harness baseline|run|diff");
        println!(
            "commands: /help, /clear, /remember, /forget, /memories, /handoff, /biscuits, /sessions, /resume, /last, /config, /shortcut, /goal, /plan, /observe, /computer-use, /mcp, /memory-mode, /privacy, /permissions"
        );
        return Ok(());
    }

    let client = Client::new();
    let mut config = llm::setup(&client).await?;
    let workspace = env::current_dir()?;
    let mut memory = MemoryStore::open(workspace.clone())?;
    let mut tools = tools::ToolRuntime::new(workspace.clone())?;
    let mut perms = PermissionGuard::open(&workspace);
    let mut history = Vec::<Msg>::new();
    let mut totals = llm::Totals::default();
    let mut line = String::new();

    println!("\n🍪 Biscuits v{}", env!("CARGO_PKG_VERSION"));
    println!("workspace: {}", env::current_dir()?.display());
    println!("memory: {}", memory.root().display());
    println!(
        "permissions: {} — {}",
        perms.mode.label(),
        perms.mode.subtitle()
    );
    println!("type /help for commands, /exit to quit\n");

    loop {
        line.clear();
        print!("you> ");
        io::stdout().flush()?;
        io::stdin().read_line(&mut line)?;

        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }
        let expanded = llm::expand_shortcut(prompt);
        let prompt = expanded.as_deref().unwrap_or(prompt);
        if matches!(prompt, "/exit" | "/quit") {
            break;
        }
        if prompt.starts_with('/') {
            if let Some(output) = perms.command_output(prompt) {
                println!("{output}");
                continue;
            }
            if let Some(output) = agent::run_slash_command(
                &client,
                &config,
                &mut memory,
                &mut tools,
                &mut history,
                prompt,
            )
            .await?
            {
                if prompt == "/clear" {
                    totals = llm::Totals::default();
                }
                println!("{output}");
                continue;
            }
        }
        perms
            .stop_requested
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let extra_context = test_harness::baseline_context(&workspace);
        let capture = agent::run_turn(
            &client,
            &config,
            &mut memory,
            &mut tools,
            &mut perms,
            &mut history,
            prompt,
            &extra_context,
            true,
        )
        .await?;
        llm::print_usage_snapshot(capture.token_usage, &mut totals);
        println!();
    }

    memory.finish(&client, &config).await?;
    config.api_key.clear();
    Ok(())
}
