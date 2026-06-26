//! An opt-in persistent bottom status bar (`BISCUITS_STATUSBAR=1`).
//!
//! It uses a terminal scroll region (DECSTBM) rather than an alt-screen TUI, so
//! all normal output keeps scrolling above the bar and the terminal's native
//! scrollback still works (scroll up to "rewind" through earlier turns). The bar
//! shows permission mode, ultracode, session tokens/turns, and uncommitted
//! lines changed. Default off so the standard REPL is untouched.

use crate::ui;
use std::io::{self, IsTerminal, Write};

/// Whether the status bar is enabled (TTY + `BISCUITS_STATUSBAR` set truthy).
pub fn enabled() -> bool {
    if !io::stdout().is_terminal() {
        return false;
    }
    matches!(
        std::env::var("BISCUITS_STATUSBAR").ok().as_deref(),
        Some("1") | Some("on") | Some("true") | Some("yes")
    )
}

pub struct StatusBar {
    active: bool,
    height: u16,
}

impl StatusBar {
    /// Reserve the bottom row by setting the scroll region to rows 1..height-1.
    pub fn new() -> Self {
        if !enabled() {
            return Self {
                active: false,
                height: 0,
            };
        }
        let (_, height) = crossterm::terminal::size().unwrap_or((80, 24));
        let bar = Self {
            active: height >= 3,
            height,
        };
        if bar.active {
            // Clear the screen, reserve the bottom row, and home the cursor so the
            // banner + REPL render inside the scroll region (rows 1..height-1) and
            // the bar lives on the last row. Done before the banner prints.
            let mut out = io::stdout();
            let _ = write!(out, "\x1b[2J");
            bar.set_region(height);
            let _ = write!(out, "\x1b[H");
            let _ = out.flush();
        }
        bar
    }

    pub fn active(&self) -> bool {
        self.active
    }

    fn set_region(&self, height: u16) {
        // Scroll region = rows 1..(height-1); the last row is left for the bar.
        let mut out = io::stdout();
        let _ = write!(out, "\x1b[1;{}r", height - 1);
        let _ = out.flush();
    }

    /// Redraw the bar with `content` (already plain text; we add subtle styling).
    /// Re-checks the terminal size so a resize re-arms the region.
    pub fn set(&mut self, content: &str) {
        if !self.active {
            return;
        }
        if let Ok((width, height)) = crossterm::terminal::size() {
            if height < 3 {
                return;
            }
            if height != self.height {
                self.height = height;
                self.set_region(height);
            }
            let line = fit(content, width as usize);
            let mut out = io::stdout();
            // Save cursor, jump to the bar row, clear it, paint, restore cursor.
            let _ = write!(
                out,
                "\x1b7\x1b[{};1H\x1b[2K{}\x1b8",
                height,
                ui::status_bar(&line)
            );
            let _ = out.flush();
        }
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for StatusBar {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut out = io::stdout();
        // Reset the scroll region and clear the bar so the shell prompt is clean.
        let _ = write!(out, "\x1b[r\x1b[{};1H\x1b[2K", self.height);
        let _ = out.flush();
    }
}

/// Truncate `content` to `width` columns (char-count approximation).
fn fit(content: &str, width: usize) -> String {
    if content.chars().count() <= width {
        content.to_string()
    } else {
        content
            .chars()
            .take(width.saturating_sub(1))
            .collect::<String>()
            + "…"
    }
}

/// Build the bar text (pure; testable). Segments are separated by " · ".
pub fn format_bar(
    mode: &str,
    ultracode: bool,
    turns: u64,
    tokens: u64,
    added: usize,
    removed: usize,
) -> String {
    let mut parts = vec![format!("mode {mode}")];
    if ultracode {
        parts.push("ultracode".to_string());
    }
    parts.push(format!("{turns} turns"));
    parts.push(format!("{tokens} tok"));
    parts.push(format!("+{added}/-{removed}"));
    parts.join("  ·  ")
}

/// Count uncommitted lines added/removed in the workspace via `git diff
/// --numstat`. Returns (0,0) when not a git repo or git is unavailable.
pub fn git_lines_changed(workspace: &std::path::Path) -> (usize, usize) {
    let mut cmd = crate::shell::command("git diff --numstat");
    cmd.current_dir(workspace);
    let Ok(output) = cmd.output() else {
        return (0, 0);
    };
    if !output.status.success() {
        return (0, 0);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let (mut added, mut removed) = (0usize, 0usize);
    for line in text.lines() {
        let mut cols = line.split('\t');
        let a = cols.next().and_then(|c| c.trim().parse::<usize>().ok());
        let r = cols.next().and_then(|c| c.trim().parse::<usize>().ok());
        added += a.unwrap_or(0);
        removed += r.unwrap_or(0);
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bar_includes_segments() {
        let bar = format_bar("Assisted", false, 3, 1200, 40, 12);
        assert!(bar.contains("mode Assisted"));
        assert!(bar.contains("3 turns"));
        assert!(bar.contains("1200 tok"));
        assert!(bar.contains("+40/-12"));
        assert!(!bar.contains("ultracode"));
    }

    #[test]
    fn format_bar_shows_ultracode_when_on() {
        assert!(format_bar("Auto", true, 1, 10, 0, 0).contains("ultracode"));
    }

    #[test]
    fn fit_truncates_with_ellipsis() {
        assert_eq!(fit("hello", 10), "hello");
        assert_eq!(fit("hello world", 5), "hell…");
    }
}
