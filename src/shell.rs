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
