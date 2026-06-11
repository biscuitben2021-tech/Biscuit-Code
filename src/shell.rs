use std::process::Command;

pub fn command(input: &str) -> Command {
    #[cfg(windows)]
    {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".to_string());
        let mut command = Command::new(shell);
        command.arg("/C").arg(input);
        command
    }

    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut command = Command::new(shell);
        command.arg("-lc").arg(input);
        command
    }
}

/// Spawn the child in its own process group so its whole subtree (the login
/// shell plus any grandchildren it forks) can be signalled together. Without
/// this, killing the child on timeout/stop leaves orphaned grandchildren
/// (dev servers, watchers) holding ports and CPU.
pub fn spawn_in_own_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// Best-effort terminate the child and, on Unix, its entire process group.
pub fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The child is its own group leader (see `spawn_in_own_group`), so its
        // pid is the negative pgid that signals the whole group.
        let pid = child.id() as i32;
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

pub fn quote(input: &str) -> String {
    #[cfg(windows)]
    {
        format!("\"{}\"", input.replace('"', "\\\""))
    }

    #[cfg(not(windows))]
    {
        format!("'{}'", input.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn quote_handles_single_quotes_for_posix_shells() {
        assert_eq!(quote("Bob's app"), "'Bob'\\''s app'");
    }

    #[cfg(windows)]
    #[test]
    fn quote_wraps_windows_arguments() {
        assert_eq!(
            quote("C:\\Program Files\\App"),
            "\"C:\\Program Files\\App\""
        );
    }
}
