//! Terminal presentation for the Biscuit Code CLI.
//!
//! Lightweight ANSI styling (no extra dependencies) that automatically disables
//! itself when output is not a TTY or `NO_COLOR` is set, plus a background
//! "thinking" spinner used while the agent plans its next step.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Whether ANSI styling should be emitted. Cached: decided once per process.
pub fn color_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal())
}

fn paint(codes: &str, text: &str) -> String {
    if color_enabled() {
        format!("\x1b[{codes}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

pub fn dim(s: &str) -> String {
    paint("2", s)
}
pub fn bold(s: &str) -> String {
    paint("1", s)
}
pub fn cyan(s: &str) -> String {
    paint("38;5;44", s)
}
pub fn green(s: &str) -> String {
    paint("38;5;42", s)
}
pub fn yellow(s: &str) -> String {
    paint("38;5;214", s)
}
pub fn red(s: &str) -> String {
    paint("38;5;203", s)
}
pub fn grey(s: &str) -> String {
    paint("38;5;245", s)
}

/// The styled prompt shown at the input line.
pub fn user_prompt() -> String {
    format!("{} ", cyan(&bold("❯")))
}

/// Header printed just before the assistant's streamed answer.
pub fn assistant_header() -> String {
    format!("{} {}", cyan(&bold("●")), bold(&cyan("biscuit")))
}

pub fn stopped(text: &str) -> String {
    format!("{} {}", yellow("■"), yellow(text))
}

pub fn error(text: &str) -> String {
    format!("{} {}", red("✗"), red(text))
}

/// The startup banner. Avoids box-drawing around variable-width content (emoji
/// widths break alignment); uses a clean left-aligned layout instead.
pub fn banner(version: &str, workspace: &str, memory: &str, mode: &str, mode_sub: &str) {
    println!();
    println!("  {}  {}", "🍪", bold(&cyan("Biscuit Code")));
    println!(
        "  {}",
        grey(&format!(
            "a fast, customizable AI coding agent · v{version}"
        ))
    );
    println!();
    println!("  {}  {}", grey("workspace"), workspace);
    println!("  {}     {}", grey("memory"), memory);
    println!(
        "  {}       {} {}",
        grey("mode"),
        bold(mode),
        grey(&format!("— {mode_sub}"))
    );
    println!();
    println!(
        "  {}",
        grey("/help for commands · Ctrl-C to interrupt · /exit to quit")
    );
    println!();
}

/// An animated spinner that runs on its own thread until stopped. Used around a
/// blocking await (e.g. the planning request) to show liveness.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    pub fn start(label: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        // Only animate on a real terminal; otherwise stay silent.
        let handle = if color_enabled() {
            let stop_c = stop.clone();
            let label = label.to_string();
            Some(thread::spawn(move || {
                let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let mut out = io::stdout();
                let mut i = 0usize;
                while !stop_c.load(Ordering::Relaxed) {
                    let frame = frames[i % frames.len()];
                    let _ = write!(out, "\r{} {}", cyan(frame), dim(&label));
                    let _ = out.flush();
                    i += 1;
                    thread::sleep(Duration::from_millis(80));
                }
                // Erase the spinner line so the next output starts clean.
                let _ = write!(out, "\r\x1b[2K");
                let _ = out.flush();
            }))
        } else {
            None
        };
        Self { stop, handle }
    }

    pub fn stop(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.finish();
    }
}
