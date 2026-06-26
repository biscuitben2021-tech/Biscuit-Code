use crossterm::{
    event::{self, Event, KeyCode},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::{
    io::{self, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Default)]
struct ActivityEntry {
    title: String,
    detail: String,
    result: Option<String>,
}

#[derive(Default)]
struct ActivityState {
    expanded: bool,
    current: Option<ActivityEntry>,
}

pub struct ActivityLog {
    requested: bool,
    raw_mode: bool,
    /// Whether we've shown the one-time "↓/↑ to expand" hint this session.
    hinted: bool,
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<ActivityState>>,
    handle: Option<JoinHandle<()>>,
}

impl ActivityLog {
    pub fn new(requested: bool) -> Self {
        Self {
            requested,
            raw_mode: false,
            hinted: false,
            stop: Arc::new(AtomicBool::new(false)),
            state: Arc::new(Mutex::new(ActivityState::default())),
            handle: None,
        }
    }

    pub fn ensure_listening(&mut self) {
        if !self.requested
            || self.raw_mode
            || !io::stdin().is_terminal()
            || !io::stdout().is_terminal()
        {
            return;
        }
        if enable_raw_mode().is_err() {
            return;
        }

        self.raw_mode = true;
        self.stop.store(false, Ordering::Relaxed);
        let stop = Arc::clone(&self.stop);
        let state = Arc::clone(&self.state);
        self.handle = Some(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match event::poll(Duration::from_millis(100)) {
                    Ok(true) => match event::read() {
                        Ok(Event::Key(key)) if key.code == KeyCode::Down => {
                            if let Ok(mut state) = state.lock() {
                                state.expanded = true;
                            }
                            print_current_detail(&state);
                        }
                        Ok(Event::Key(key)) if key.code == KeyCode::Up => {
                            if let Ok(mut state) = state.lock() {
                                state.expanded = false;
                            }
                            write_raw_block("[activity collapsed]");
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    },
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        }));
    }

    pub fn stop_listening(&mut self) {
        if !self.raw_mode {
            return;
        }
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let _ = disable_raw_mode();
        self.raw_mode = false;
    }

    pub fn tool_started(&mut self, title: impl Into<String>, detail: impl Into<String>) {
        let entry = ActivityEntry {
            title: title.into(),
            detail: sanitize(&detail.into()),
            result: None,
        };
        if let Ok(mut state) = self.state.lock() {
            state.current = Some(entry.clone());
        }

        self.write_block(&format!(
            "\n{}  {}",
            crate::ui::cyan("⏺"),
            crate::ui::bold(&entry.title)
        ));
        if !self.hinted {
            self.hinted = true;
            self.write_block(&crate::ui::dim("   ↓/↑ expand or collapse tool details"));
        }
        if self.expanded() {
            self.write_block(&format!(
                "{}\n{}",
                crate::ui::dim("   details:"),
                entry.detail
            ));
        }
    }

    pub fn tool_finished(&mut self, brief: &str, result: &str) {
        // Sanitize untrusted tool output before printing so a crafted ANSI/escape
        // sequence in a file, web page, or command result can't rewrite the
        // operator's terminal.
        let result = sanitize(&truncate(result, 8_000));
        let brief = sanitize(brief);
        if let Ok(mut state) = self.state.lock() {
            if let Some(current) = &mut state.current {
                current.result = Some(result.clone());
            }
        }

        self.write_block(&format!(
            "  {} {}",
            crate::ui::green("⎿"),
            crate::ui::dim(brief.trim())
        ));
        if self.expanded() {
            self.write_block(&format!(
                "{}\n{}",
                crate::ui::dim("   result:"),
                result.trim()
            ));
        }
    }

    fn expanded(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.expanded)
            .unwrap_or(false)
    }

    fn write_block(&self, text: &str) {
        if self.raw_mode {
            write_raw_block(text);
            return;
        }
        let mut out = io::stdout();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }
}

impl Drop for ActivityLog {
    fn drop(&mut self) {
        self.stop_listening();
    }
}

fn print_current_detail(state: &Arc<Mutex<ActivityState>>) {
    let entry = state.lock().ok().and_then(|state| state.current.clone());
    let Some(entry) = entry else {
        write_raw_block("[activity] no active tool yet");
        return;
    };

    let mut block = format!("[activity expanded]\n{}", entry.detail);
    if let Some(result) = entry.result {
        block.push_str("\n\nresult details:\n");
        block.push_str(result.trim());
    }
    write_raw_block(&block);
}

fn write_raw_block(text: &str) {
    let mut out = io::stdout();
    let text = text.replace('\n', "\r\n");
    let _ = write!(out, "\r\n{text}\r\n");
    let _ = out.flush();
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

/// Strip control characters (notably ESC, which starts ANSI sequences) from
/// untrusted tool output, keeping newlines and tabs. Prevents terminal-escape
/// injection from file contents, web pages, and command output.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect()
}
