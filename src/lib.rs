mod activity;
mod agent;
mod browser;
mod computer_use;
mod evals;
mod goals;
mod llm;
mod mcp;
mod memory;
mod observations;
mod permissions;
mod shell;
mod skills;
mod test_harness;
mod tools;
mod ui;

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
            "commands: /help, /clear, /remember, /forget, /memories, /handoff, /biscuits, /sessions, /resume, /last, /config, /shortcut, /goal, /plan, /observe, /computer-use, /browser, /mcp, /skills, /memory-mode, /privacy, /permissions"
        );
        return Ok(());
    }

    let client = Client::new();
    let mut config = llm::setup(&client).await?;
    let workspace = env::current_dir()?;
    let mut memory = MemoryStore::open(workspace.clone())?;
    let mut tools = tools::ToolRuntime::new(workspace.clone())?;
    let mut perms = PermissionGuard::open(&workspace);

    // Ctrl-C handling: during a turn it requests a halt that the agent loop
    // polls; at the idle prompt it exits the process. It runs on a worker thread
    // so it still fires while the main task is blocked on stdin (the default
    // multi-thread runtime makes this work).
    let turn_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let stop_flag = perms.stop_requested.clone();
        let active = turn_active.clone();
        tokio::spawn(async move {
            use std::sync::atomic::Ordering::Relaxed;
            while tokio::signal::ctrl_c().await.is_ok() {
                if active.load(Relaxed) {
                    stop_flag.store(true, Relaxed);
                    eprintln!("\n[stopping] halting after the current step…");
                } else {
                    eprintln!();
                    std::process::exit(0);
                }
            }
        });
    }

    let mut history = Vec::<Msg>::new();
    let mut totals = llm::Totals::default();
    let mut line = String::new();

    ui::banner(
        env!("CARGO_PKG_VERSION"),
        &env::current_dir()?.display().to_string(),
        &memory.root().display().to_string(),
        perms.mode.label(),
        perms.mode.subtitle(),
    );

    loop {
        line.clear();
        print!("{}", ui::user_prompt());
        io::stdout().flush()?;
        let read = io::stdin().read_line(&mut line)?;
        if read == 0 {
            // EOF (Ctrl-D or closed/piped stdin): exit cleanly instead of
            // spinning forever on an empty prompt.
            println!();
            break;
        }

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
        turn_active.store(true, std::sync::atomic::Ordering::Relaxed);
        // A turn error (transient API failure, malformed model output, a snapshot
        // I/O hiccup) must not crash the whole REPL and discard the session —
        // report it and keep going.
        let turn_result = agent::run_turn(
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
        .await;
        turn_active.store(false, std::sync::atomic::Ordering::Relaxed);
        match turn_result {
            Ok(capture) => {
                llm::print_usage_snapshot(capture.token_usage, &mut totals);
            }
            Err(err) => {
                eprintln!("\n{}", ui::error(&err.to_string()));
            }
        }
        println!();
    }

    memory.finish(&client, &config).await?;
    config.api_key.clear();
    Ok(())
}
