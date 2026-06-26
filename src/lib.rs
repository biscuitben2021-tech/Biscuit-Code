mod activity;
mod agent;
mod browser;
mod computer_use;
mod contract;
mod evals;
mod goals;
pub mod gui;
mod hooks;
mod input;
mod llm;
mod markdown;
mod mcp;
mod memory;
mod observations;
mod permissions;
mod shell;
mod skills;
mod statusbar;
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
    // One-time, best-effort migration: the per-workspace state folder used to be
    // hidden (`.biscuits`). Make it visible (`biscuits`) by renaming it in place,
    // preserving all existing user state. Only migrate when the old folder exists
    // and the new one does not, so we never clobber or delete anything.
    if let Ok(ws) = std::env::current_dir() {
        if ws.join(".biscuits").exists() && !ws.join("biscuits").exists() {
            let _ = std::fs::rename(ws.join(".biscuits"), ws.join("biscuits"));
        }
    }

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
            "commands: /help, /clear, /remember, /forget, /memories, /handoff, /biscuits, /sessions, /resume, /last, /config, /shortcut, /goal, /plan, /observe, /computer-use, /browser, /mcp, /plugins, /install, /skills, /test, /ultracode, /focus, /configure, /memory-mode, /privacy, /permissions"
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
                    // process::exit skips Drop, so reset any status-bar scroll
                    // region here — otherwise the shell prompt is left stuck
                    // inside the reserved region after Ctrl-C.
                    print!("\x1b[r");
                    let _ = io::stdout().flush();
                    eprintln!();
                    std::process::exit(0);
                }
            }
        });
    }

    let mut history = Vec::<Msg>::new();
    let mut totals = llm::Totals::default();
    let mut line = String::new();

    // Opt-in persistent bottom status bar (BISCUITS_STATUSBAR=1). Created before
    // the banner so its scroll region is in place when the banner prints.
    let mut status = statusbar::StatusBar::new();

    ui::banner(
        env!("CARGO_PKG_VERSION"),
        &env::current_dir()?.display().to_string(),
        &memory.root().display().to_string(),
        perms.mode.label(),
        perms.mode.subtitle(),
    );
    if tools.ultracode() {
        println!("  {}\n", ui::ultracode_badge());
    }
    let draw_status = |status: &mut statusbar::StatusBar,
                       totals: &llm::Totals,
                       tools: &tools::ToolRuntime,
                       perms: &PermissionGuard| {
        if status.active() {
            let (added, removed) = statusbar::git_lines_changed(&workspace);
            status.set(&statusbar::format_bar(
                perms.mode.label(),
                tools.ultracode(),
                totals.turns(),
                totals.tokens(),
                added,
                removed,
            ));
        }
    };
    draw_status(&mut status, &totals, &tools, &perms);

    loop {
        line.clear();
        // Opt-in bordered input box (BISCUITS_INPUT=box). When it returns
        // Some(text), use it as the typed line (newline-terminated to match the
        // read_line contract). When it returns None, fall back to the plain
        // read_line path for this iteration — and if the flag is off, we never
        // call it at all, so the default behavior is byte-for-byte unchanged.
        let mut handled_by_box = false;
        if input::box_enabled() {
            if let Some(text) = input::read_line_box(&ui::user_prompt())? {
                line.push_str(&text);
                line.push('\n');
                handled_by_box = true;
            }
        }
        if !handled_by_box {
            print!("{}", ui::user_prompt());
            io::stdout().flush()?;
            let read = io::stdin().read_line(&mut line)?;
            if read == 0 {
                // EOF (Ctrl-D or closed/piped stdin): exit cleanly instead of
                // spinning forever on an empty prompt.
                println!();
                break;
            }
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
            if prompt == "/configure" || prompt == "/settings" {
                let mut out = ui::bold(&ui::cyan("Biscuit Code — configuration"));
                out.push('\n');
                out.push_str(&format!(
                    "  provider   {}\n",
                    llm::provider_name(config.provider)
                ));
                out.push_str(&format!("  model      {}\n", config.model));
                out.push_str(&format!(
                    "  mode       {} — {}\n",
                    perms.mode.label(),
                    perms.mode.subtitle()
                ));
                out.push_str(&format!(
                    "  ultracode  {}\n",
                    if tools.ultracode() { "on" } else { "off" }
                ));
                out.push_str(&format!("  focus      {}\n", tools.focus_display()));
                out.push_str(&format!("  state      {}\n", memory.root().display()));
                for cmd in ["/skills", "/mcp list", "/plugins"] {
                    if let Ok(Some(section)) = tools.command_output(cmd) {
                        out.push('\n');
                        out.push_str(section.trim_end());
                        out.push('\n');
                    }
                }
                out.push_str(&ui::grey(
                    "\nchange: /permissions  /memory-mode  /privacy  /ultracode  /skills  /mcp  /plugins  /focus",
                ));
                println!("{out}");
                continue;
            }
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
        // Re-render the just-typed message as a right-aligned chat bubble.
        ui::echo_user(prompt);
        let extra_context = test_harness::baseline_context(&workspace);
        turn_active.store(true, std::sync::atomic::Ordering::Relaxed);
        // A turn error (transient API failure, malformed model output, a snapshot
        // I/O hiccup) must not crash the whole REPL and discard the session —
        // report it and keep going. The per-turn token line is printed inside the
        // turn (right after the answer) so it can't interrupt the next message.
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
            0,
            &mut totals,
        )
        .await;
        turn_active.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Err(err) = turn_result {
            eprintln!("\n{}", ui::error(&err.to_string()));
        }
        draw_status(&mut status, &totals, &tools, &perms);
        println!();
    }

    memory.finish(&client, &config).await?;
    config.api_key.clear();
    Ok(())
}
