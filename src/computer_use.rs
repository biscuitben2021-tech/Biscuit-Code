use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct ComputerUseRuntime {
    workspace: PathBuf,
    screenshot_dir: PathBuf,
    last_target: Option<String>,
    last_screenshot: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct KeyStroke {
    code: u16,
    flags: u64,
}

const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_ALT: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

impl ComputerUseRuntime {
    pub fn new(workspace: PathBuf) -> Self {
        let workspace = workspace.canonicalize().unwrap_or(workspace);
        let screenshot_dir = workspace.join(".biscuits/computer_use/screenshots");
        Self {
            workspace,
            screenshot_dir,
            last_target: None,
            last_screenshot: None,
        }
    }

    pub fn help() -> &'static str {
        r#"computer use:
  /computer-use screenshot
  /computer-use open <url-or-path>
  /computer-use app <application-name>
  /computer-use run <shell-command>
  /computer-use click <x> <y> [left|right|middle]
  /computer-use move <x> <y>
  /computer-use type <text>
  /computer-use key <key-or-combo>
  /computer-use wait <milliseconds>
  /computer-use state

The ComputerUse tool gives the agent the same UI loop: open an app or website,
capture a screenshot, then move/click/type in screen pixel coordinates and
screenshot again for verification. On macOS, grant Screen Recording and
Accessibility permissions to the terminal app if screenshots or clicks are
blocked by the OS."#
    }

    pub fn command_output(&mut self, input: &str) -> Result<Option<String>> {
        let input = input.trim();
        if input == "/computer-use" || input == "/computer-use help" {
            return Ok(Some(Self::help().into()));
        }
        let Some(rest) = input.strip_prefix("/computer-use ") else {
            return Ok(None);
        };
        let rest = rest.trim();
        if rest.is_empty() {
            return Ok(Some(Self::help().into()));
        }

        if rest == "screenshot" {
            return Ok(Some(self.execute(&json!({ "action": "screenshot" }))?));
        }
        if rest == "state" {
            return Ok(Some(self.state()));
        }
        if let Some(target) = rest.strip_prefix("open ") {
            return Ok(Some(self.execute(&json!({
                "action": "open",
                "target": target.trim()
            }))?));
        }
        if let Some(app) = rest.strip_prefix("app ") {
            return Ok(Some(self.execute(&json!({
                "action": "open",
                "app": app.trim()
            }))?));
        }
        if let Some(command) = rest.strip_prefix("run ") {
            return Ok(Some(self.execute(&json!({
                "action": "open",
                "command": command.trim()
            }))?));
        }
        if let Some(text) = rest.strip_prefix("type ") {
            return Ok(Some(self.execute(&json!({
                "action": "type",
                "text": text
            }))?));
        }
        if let Some(key) = rest.strip_prefix("key ") {
            return Ok(Some(self.execute(&json!({
                "action": "key",
                "key": key.trim()
            }))?));
        }
        if let Some(ms) = rest.strip_prefix("wait ") {
            let wait_ms = ms
                .trim()
                .parse::<u64>()
                .context("wait requires milliseconds")?;
            return Ok(Some(self.execute(&json!({
                "action": "wait",
                "wait_ms": wait_ms
            }))?));
        }
        if let Some(args) = rest.strip_prefix("click ") {
            let parts = args.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 2 {
                bail!("usage: /computer-use click <x> <y> [left|right|middle]");
            }
            return Ok(Some(self.execute(&json!({
                "action": "click",
                "x": parse_f64(parts[0], "x")?,
                "y": parse_f64(parts[1], "y")?,
                "button": parts.get(2).copied().unwrap_or("left")
            }))?));
        }
        if let Some(args) = rest.strip_prefix("move ") {
            let parts = args.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 2 {
                bail!("usage: /computer-use move <x> <y>");
            }
            return Ok(Some(self.execute(&json!({
                "action": "move",
                "x": parse_f64(parts[0], "x")?,
                "y": parse_f64(parts[1], "y")?
            }))?));
        }

        Ok(Some(format!(
            "unknown /computer-use command\n\n{}",
            Self::help()
        )))
    }

    pub fn execute(&mut self, args: &Value) -> Result<String> {
        match str_arg(args, "action")
            .or_else(|_| str_arg(args, "type"))
            .unwrap_or("screenshot")
            .to_lowercase()
            .as_str()
        {
            "screenshot" | "view" | "observe" => self.screenshot_action("manual"),
            "open" | "launch" => self.open_action(args),
            "click" => self.click_action(args),
            "move" | "mousemove" => self.move_action(args),
            "type" | "text" => self.type_action(args),
            "key" | "keypress" => self.key_action(args),
            "wait" | "sleep" => self.wait_action(args),
            "state" => Ok(self.state()),
            other => bail!("unknown ComputerUse action: {other}"),
        }
    }

    fn open_action(&mut self, args: &Value) -> Result<String> {
        let command = self.open_command(args)?;
        let wait_ms = u64_arg(args, "wait_ms", 1_500).min(30_000);
        let output = run_shell(&self.workspace, &command, Duration::from_secs(30))?;
        thread::sleep(Duration::from_millis(wait_ms));
        let screenshot = self.capture_screenshot("open");

        let mut out = format!(
            "action: open\ncommand: {command}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            truncate(&output.stdout, 4_000),
            truncate(&output.stderr, 4_000)
        );
        append_screenshot_result(&mut out, screenshot);
        Ok(out)
    }

    fn screenshot_action(&mut self, label: &str) -> Result<String> {
        let path = self.capture_screenshot(label)?;
        Ok(format!(
            "action: screenshot\nscreenshot: {}\nbytes: {}",
            path.display(),
            fs::metadata(&path)?.len()
        ))
    }

    fn click_action(&mut self, args: &Value) -> Result<String> {
        let x = number_arg(args, "x")?;
        let y = number_arg(args, "y")?;
        let button = Button::from_str(str_arg(args, "button").unwrap_or("left"))?;
        let clicks = u64_arg(args, "clicks", 1).clamp(1, 3);
        let interval_ms = u64_arg(args, "interval_ms", 80).min(1_000);
        os_input::click(x, y, button, clicks, interval_ms)?;
        let screenshot_after = bool_arg(args, "screenshot", true);

        let mut out = format!("action: click\nx: {x}\ny: {y}\nbutton: {button}\nclicks: {clicks}");
        if screenshot_after {
            append_screenshot_result(&mut out, self.capture_screenshot("click"));
        }
        Ok(out)
    }

    fn move_action(&mut self, args: &Value) -> Result<String> {
        let x = number_arg(args, "x")?;
        let y = number_arg(args, "y")?;
        os_input::move_cursor(x, y)?;
        let screenshot_after = bool_arg(args, "screenshot", true);

        let mut out = format!("action: move\nx: {x}\ny: {y}");
        if screenshot_after {
            append_screenshot_result(&mut out, self.capture_screenshot("move"));
        }
        Ok(out)
    }

    fn type_action(&mut self, args: &Value) -> Result<String> {
        let text = str_arg(args, "text").or_else(|_| str_arg(args, "value"))?;
        os_input::type_text(text)?;
        let screenshot_after = bool_arg(args, "screenshot", true);

        let mut out = format!("action: type\nchars: {}", text.chars().count());
        if screenshot_after {
            append_screenshot_result(&mut out, self.capture_screenshot("type"));
        }
        Ok(out)
    }

    fn key_action(&mut self, args: &Value) -> Result<String> {
        let key = str_arg(args, "key").or_else(|_| str_arg(args, "value"))?;
        let stroke = parse_key_stroke(key)?;
        os_input::press_key(stroke)?;
        let screenshot_after = bool_arg(args, "screenshot", true);

        let mut out = format!("action: key\nkey: {key}");
        if screenshot_after {
            append_screenshot_result(&mut out, self.capture_screenshot("key"));
        }
        Ok(out)
    }

    fn wait_action(&mut self, args: &Value) -> Result<String> {
        let wait_ms = u64_arg(args, "wait_ms", 1_000).min(60_000);
        thread::sleep(Duration::from_millis(wait_ms));
        let screenshot_after = bool_arg(args, "screenshot", true);

        let mut out = format!("action: wait\nwait_ms: {wait_ms}");
        if screenshot_after {
            append_screenshot_result(&mut out, self.capture_screenshot("wait"));
        }
        Ok(out)
    }

    fn state(&self) -> String {
        format!(
            "computer_use_state:\nlast_target: {}\nlast_screenshot: {}\nscreenshot_dir: {}",
            self.last_target.as_deref().unwrap_or("none"),
            self.last_screenshot
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "none".into()),
            self.screenshot_dir.display()
        )
    }

    fn open_command(&mut self, args: &Value) -> Result<String> {
        if let Ok(command) = str_arg(args, "command") {
            self.last_target = Some(command.to_string());
            return Ok(command.to_string());
        }
        if let Ok(app) = str_arg(args, "app") {
            self.last_target = Some(app.to_string());
            return Ok(platform_open_app_command(app));
        }
        let target = str_arg(args, "target")
            .or_else(|_| str_arg(args, "url"))
            .or_else(|_| str_arg(args, "path"))?;
        let resolved = if args.get("path").is_some() {
            self.resolve_open_path(target)?
        } else {
            target.to_string()
        };
        self.last_target = Some(resolved.clone());
        Ok(platform_open_target_command(&resolved))
    }

    fn resolve_open_path(&self, path: &str) -> Result<String> {
        let path = Path::new(path);
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        };
        Ok(full
            .canonicalize()
            .with_context(|| format!("cannot resolve path: {}", full.display()))?
            .to_string_lossy()
            .to_string())
    }

    fn capture_screenshot(&mut self, label: &str) -> Result<PathBuf> {
        fs::create_dir_all(&self.screenshot_dir)?;
        let path = self
            .screenshot_dir
            .join(format!("{}-{}.png", safe_label(label), now_millis()));
        capture_screenshot_to(&path)?;
        let bytes = fs::metadata(&path)
            .with_context(|| {
                format!(
                    "screenshot file was not created; grant Screen Recording permission if prompted: {}",
                    path.display()
                )
            })?
            .len();
        if bytes == 0 {
            bail!("screenshot file is empty: {}", path.display());
        }
        self.last_screenshot = Some(path.clone());
        Ok(path)
    }
}

impl std::fmt::Display for Button {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Button::Left => write!(f, "left"),
            Button::Right => write!(f, "right"),
            Button::Middle => write!(f, "middle"),
        }
    }
}

impl Button {
    fn from_str(input: &str) -> Result<Self> {
        match input.to_lowercase().as_str() {
            "left" | "primary" => Ok(Self::Left),
            "right" | "secondary" => Ok(Self::Right),
            "middle" | "center" => Ok(Self::Middle),
            other => bail!("unknown mouse button: {other}"),
        }
    }
}

struct ShellRun {
    status: String,
    stdout: String,
    stderr: String,
}

fn run_shell(workspace: &Path, command: &str, timeout: Duration) -> Result<ShellRun> {
    let mut child = crate::shell::command(command)
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run computer-use command: {command}"))?;

    let start = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let output = child.wait_with_output()?;
    Ok(ShellRun {
        status: output.status.to_string(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn append_screenshot_result(out: &mut String, screenshot: Result<PathBuf>) {
    match screenshot {
        Ok(path) => out.push_str(&format!("\nscreenshot: {}", path.display())),
        Err(err) => out.push_str(&format!("\nscreenshot_error: {err}")),
    }
}

fn capture_screenshot_to(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("screencapture")
            .arg("-x")
            .arg(path)
            .output()
            .context("failed to run screencapture")?;
        if !output.status.success() {
            bail!(
                "screencapture failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        for candidate in [
            vec!["gnome-screenshot", "-f"],
            vec!["grim"],
            vec!["import", "-window", "root"],
        ] {
            let Some(program) = candidate.first() else {
                continue;
            };
            if !command_exists(program) {
                continue;
            }
            let mut command = std::process::Command::new(program);
            for arg in candidate.iter().skip(1) {
                command.arg(arg);
            }
            command.arg(path);
            let output = command.output()?;
            if output.status.success() {
                return Ok(());
            }
        }
        bail!("no supported screenshot command found; install gnome-screenshot, grim, or ImageMagick import")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        bail!("screenshots are supported on macOS and Linux in this build")
    }
}

#[cfg(target_os = "linux")]
fn command_exists(program: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {}", crate::shell::quote(program)))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn platform_open_target_command(target: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        format!("open {}", crate::shell::quote(target))
    }

    #[cfg(target_os = "linux")]
    {
        format!("xdg-open {}", crate::shell::quote(target))
    }

    #[cfg(windows)]
    {
        format!("start \"\" {}", crate::shell::quote(target))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        format!("open {}", crate::shell::quote(target))
    }
}

fn platform_open_app_command(app: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        format!("open -a {}", crate::shell::quote(app))
    }

    #[cfg(target_os = "linux")]
    {
        crate::shell::quote(app)
    }

    #[cfg(windows)]
    {
        format!("start \"\" {}", crate::shell::quote(app))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        format!("open -a {}", crate::shell::quote(app))
    }
}

fn parse_key_stroke(input: &str) -> Result<KeyStroke> {
    let mut flags = 0;
    let mut key_code = None;

    for raw in input.split('+') {
        let part = raw.trim().to_lowercase();
        if part.is_empty() {
            continue;
        }
        match part.as_str() {
            "shift" => flags |= FLAG_SHIFT,
            "ctrl" | "control" => flags |= FLAG_CONTROL,
            "alt" | "option" => flags |= FLAG_ALT,
            "cmd" | "command" | "meta" | "super" => flags |= FLAG_COMMAND,
            key => key_code = Some(key_code_for(key)?),
        }
    }

    Ok(KeyStroke {
        code: key_code.context("key combo requires a non-modifier key")?,
        flags,
    })
}

fn key_code_for(key: &str) -> Result<u16> {
    let code = match key {
        "a" => 0,
        "s" => 1,
        "d" => 2,
        "f" => 3,
        "h" => 4,
        "g" => 5,
        "z" => 6,
        "x" => 7,
        "c" => 8,
        "v" => 9,
        "b" => 11,
        "q" => 12,
        "w" => 13,
        "e" => 14,
        "r" => 15,
        "y" => 16,
        "t" => 17,
        "1" => 18,
        "2" => 19,
        "3" => 20,
        "4" => 21,
        "6" => 22,
        "5" => 23,
        "=" | "equal" => 24,
        "9" => 25,
        "7" => 26,
        "-" | "minus" => 27,
        "8" => 28,
        "0" => 29,
        "]" | "rightbracket" => 30,
        "o" => 31,
        "u" => 32,
        "[" | "leftbracket" => 33,
        "i" => 34,
        "p" => 35,
        "return" | "enter" => 36,
        "l" => 37,
        "j" => 38,
        "'" | "quote" => 39,
        "k" => 40,
        ";" | "semicolon" => 41,
        "\\" | "backslash" => 42,
        "," | "comma" => 43,
        "/" | "slash" => 44,
        "n" => 45,
        "m" => 46,
        "." | "period" => 47,
        "tab" => 48,
        "space" => 49,
        "`" | "grave" => 50,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "home" => 115,
        "pageup" | "page-up" => 116,
        "end" => 119,
        "pagedown" | "page-down" => 121,
        "left" | "arrowleft" => 123,
        "right" | "arrowright" => 124,
        "down" | "arrowdown" => 125,
        "up" | "arrowup" => 126,
        other => bail!("unknown key: {other}"),
    };
    Ok(code)
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string arg: {key}"))
}

fn number_arg(args: &Value, key: &str) -> Result<f64> {
    args.get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("missing number arg: {key}"))
}

fn bool_arg(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn u64_arg(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn parse_f64(input: &str, name: &str) -> Result<f64> {
    input
        .parse::<f64>()
        .with_context(|| format!("{name} must be a number"))
}

fn safe_label(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

#[cfg(target_os = "macos")]
mod os_input {
    use super::{Button, KeyStroke};
    use anyhow::{bail, Result};
    use std::{ffi::c_void, ptr, thread, time::Duration};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    type CGEventRef = *mut c_void;
    type CGEventSourceRef = *mut c_void;

    const K_CG_HID_EVENT_TAP: u32 = 0;
    const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
    const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
    const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
    const K_CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
    const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
    const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
    const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateMouseEvent(
            source: CGEventSourceRef,
            mouse_type: u32,
            mouse_cursor_position: CGPoint,
            mouse_button: u32,
        ) -> CGEventRef;
        fn CGEventCreateKeyboardEvent(
            source: CGEventSourceRef,
            virtual_key: u16,
            key_down: bool,
        ) -> CGEventRef;
        fn CGEventKeyboardSetUnicodeString(
            event: CGEventRef,
            string_length: usize,
            unicode_string: *const u16,
        );
        fn CGEventSetFlags(event: CGEventRef, flags: u64);
        fn CGEventPost(tap: u32, event: CGEventRef);
        fn CFRelease(cf: *const c_void);
    }

    pub fn move_cursor(x: f64, y: f64) -> Result<()> {
        post_mouse(K_CG_EVENT_MOUSE_MOVED, x, y, 0)
    }

    pub fn click(x: f64, y: f64, button: Button, clicks: u64, interval_ms: u64) -> Result<()> {
        move_cursor(x, y)?;
        let (down, up, button_id) = match button {
            Button::Left => (K_CG_EVENT_LEFT_MOUSE_DOWN, K_CG_EVENT_LEFT_MOUSE_UP, 0),
            Button::Right => (K_CG_EVENT_RIGHT_MOUSE_DOWN, K_CG_EVENT_RIGHT_MOUSE_UP, 1),
            Button::Middle => (K_CG_EVENT_OTHER_MOUSE_DOWN, K_CG_EVENT_OTHER_MOUSE_UP, 2),
        };
        for _ in 0..clicks {
            post_mouse(down, x, y, button_id)?;
            thread::sleep(Duration::from_millis(25));
            post_mouse(up, x, y, button_id)?;
            thread::sleep(Duration::from_millis(interval_ms));
        }
        Ok(())
    }

    pub fn type_text(text: &str) -> Result<()> {
        for ch in text.chars() {
            let mut buf = [0_u16; 2];
            let encoded = ch.encode_utf16(&mut buf);
            post_unicode_key(encoded)?;
            thread::sleep(Duration::from_millis(4));
        }
        Ok(())
    }

    pub fn press_key(stroke: KeyStroke) -> Result<()> {
        post_key(stroke.code, stroke.flags, true)?;
        thread::sleep(Duration::from_millis(20));
        post_key(stroke.code, stroke.flags, false)
    }

    fn post_mouse(event_type: u32, x: f64, y: f64, button: u32) -> Result<()> {
        let event = unsafe {
            CGEventCreateMouseEvent(ptr::null_mut(), event_type, CGPoint { x, y }, button)
        };
        post_and_release(event)
    }

    fn post_unicode_key(units: &[u16]) -> Result<()> {
        let down = unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), 0, true) };
        if down.is_null() {
            bail!("failed to create keyboard event");
        }
        unsafe {
            CGEventKeyboardSetUnicodeString(down, units.len(), units.as_ptr());
            CGEventPost(K_CG_HID_EVENT_TAP, down);
            CFRelease(down as *const c_void);
        }

        let up = unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), 0, false) };
        if up.is_null() {
            bail!("failed to create keyboard event");
        }
        unsafe {
            CGEventKeyboardSetUnicodeString(up, units.len(), units.as_ptr());
            CGEventPost(K_CG_HID_EVENT_TAP, up);
            CFRelease(up as *const c_void);
        }
        Ok(())
    }

    fn post_key(code: u16, flags: u64, key_down: bool) -> Result<()> {
        let event = unsafe { CGEventCreateKeyboardEvent(ptr::null_mut(), code, key_down) };
        if event.is_null() {
            bail!("failed to create keyboard event");
        }
        unsafe {
            CGEventSetFlags(event, flags);
            CGEventPost(K_CG_HID_EVENT_TAP, event);
            CFRelease(event as *const c_void);
        }
        Ok(())
    }

    fn post_and_release(event: CGEventRef) -> Result<()> {
        if event.is_null() {
            bail!("failed to create mouse event; grant Accessibility permission if prompted");
        }
        unsafe {
            CGEventPost(K_CG_HID_EVENT_TAP, event);
            CFRelease(event as *const c_void);
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod os_input {
    use super::{Button, KeyStroke};
    use anyhow::{bail, Result};

    pub fn move_cursor(_x: f64, _y: f64) -> Result<()> {
        bail!("cursor control is currently implemented for macOS builds")
    }

    pub fn click(_x: f64, _y: f64, _button: Button, _clicks: u64, _interval_ms: u64) -> Result<()> {
        bail!("cursor control is currently implemented for macOS builds")
    }

    pub fn type_text(_text: &str) -> Result<()> {
        bail!("typing is currently implemented for macOS builds")
    }

    pub fn press_key(_stroke: KeyStroke) -> Result<()> {
        bail!("keyboard control is currently implemented for macOS builds")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_command_key_combo() {
        let stroke = parse_key_stroke("cmd+l").unwrap();

        assert_eq!(stroke.code, 37);
        assert_eq!(stroke.flags, FLAG_COMMAND);
    }

    #[test]
    fn help_command_is_available_without_screen_access() {
        let mut runtime = ComputerUseRuntime::new(std::env::temp_dir());

        let output = runtime.command_output("/computer-use").unwrap().unwrap();

        assert!(output.contains("/computer-use screenshot"));
    }
}
