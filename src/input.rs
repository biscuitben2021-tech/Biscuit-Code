//! An opt-in, two-line auto-expanding bordered input box for the REPL prompt
//! (`BISCUITS_INPUT=box`).
//!
//! The module is split in two on purpose:
//!
//!  * [`InputEditor`] and [`render_box`] are a *pure* editor core — no terminal
//!    I/O at all — so the tricky parts (UTF-8 cursor handling, wrapping,
//!    auto-expand, cursor placement) are fully unit-tested below.
//!  * [`read_line_box`] is a *thin* raw-mode terminal reader that wires the core
//!    to a real TTY. It can't be exercised headlessly, so it stays small and
//!    delegates every editing decision to [`InputEditor`].
//!
//! When the env flag is off (the default), the caller never touches this module
//! and the original `read_line` path is used unchanged.

use std::io::{self, IsTerminal, Read, Write};

/// The minimum number of content rows the box ever shows (a "two-line" box).
const MIN_ROWS: usize = 2;
/// The maximum number of content rows before the view starts scrolling.
const MAX_ROWS: usize = 10;
/// Smallest total width we will draw at; below this the content area would
/// vanish, so we clamp up to keep at least one usable content column.
const MIN_WIDTH: usize = 5;

/// A pure text editor: a buffer plus a cursor expressed as a **char** index
/// (not a byte offset), so every operation is UTF-8 safe and never splits a
/// multi-byte character. Contains no terminal state whatsoever.
#[derive(Debug, Default, Clone)]
pub struct InputEditor {
    buf: String,
    /// Cursor position as a char index in `0..=char_count`.
    cursor: usize,
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            buf: String::new(),
            cursor: 0,
        }
    }

    /// Number of characters (not bytes) in the buffer.
    fn len_chars(&self) -> usize {
        self.buf.chars().count()
    }

    /// Byte offset of char index `idx` (clamped to the buffer length).
    fn byte_at(&self, idx: usize) -> usize {
        self.buf
            .char_indices()
            .nth(idx)
            .map(|(b, _)| b)
            .unwrap_or(self.buf.len())
    }

    /// The current text.
    pub fn text(&self) -> &str {
        &self.buf
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// The cursor position as a char index.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Insert one character at the cursor and advance the cursor past it.
    pub fn insert_char(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.buf.insert(at, c);
        self.cursor += 1;
    }

    /// Insert a whole string at the cursor (used for pasted / multi-byte input).
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
    }

    /// Delete the character *before* the cursor (Backspace). No-op at the start.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.buf.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Delete the character *at* the cursor (Delete). No-op at the end.
    pub fn delete(&mut self) {
        if self.cursor >= self.len_chars() {
            return;
        }
        let start = self.byte_at(self.cursor);
        let end = self.byte_at(self.cursor + 1);
        self.buf.replace_range(start..end, "");
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.len_chars() {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.len_chars();
    }

    /// Delete the word before the cursor (Ctrl-W): skip any whitespace directly
    /// before the cursor, then delete the run of non-whitespace before that.
    pub fn delete_word(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let chars: Vec<char> = self.buf.chars().collect();
        let mut start = self.cursor;
        // Skip trailing whitespace immediately before the cursor.
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        // Then delete the contiguous non-whitespace word.
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let start_byte = self.byte_at(start);
        let end_byte = self.byte_at(self.cursor);
        self.buf.replace_range(start_byte..end_byte, "");
        self.cursor = start;
    }

    /// Reset to an empty buffer. Part of the public editor API; the raw-mode
    /// reader builds a fresh editor per line rather than reusing one, so this is
    /// exercised by tests and available to other callers.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
    }
}

/// The fully-rendered box: the literal lines to print (top border, content
/// rows, bottom border) plus where the real terminal cursor should land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedBox {
    /// Every line of the box, top border first, bottom border last.
    pub lines: Vec<String>,
    /// Row index of the cursor *within `lines`* (so the first content row is 1,
    /// because `lines[0]` is the top border).
    pub cursor_row: usize,
    /// Column of the cursor within that line (0-based, counting display chars
    /// from the left edge of the line, i.e. including the `│ ` prefix).
    pub cursor_col: usize,
}

/// Wrap `text` into rows of at most `content_width` chars, tracking which row
/// and column the cursor (a char index) lands on. Returns `(rows, cur_row,
/// cur_col)` where `cur_col` is the column *within the content area* (0-based).
///
/// Wrapping is by hard character count (no word breaking) so it is fully
/// deterministic. A cursor sitting exactly at a wrap boundary is reported at the
/// start of the next row, except at the very end of the text where it stays on
/// the last row.
fn wrap_with_cursor(
    text: &str,
    cursor: usize,
    content_width: usize,
) -> (Vec<String>, usize, usize) {
    let width = content_width.max(1);
    let chars: Vec<char> = text.chars().collect();

    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    let mut col = 0usize;
    let mut cur_row = 0usize;
    let mut cur_col = 0usize;
    let mut cursor_placed = false;

    for (i, &c) in chars.iter().enumerate() {
        // Place the cursor before consuming the char it sits on.
        if i == cursor {
            cur_row = rows.len();
            cur_col = col;
            cursor_placed = true;
        }
        row.push(c);
        col += 1;
        if col == width {
            rows.push(std::mem::take(&mut row));
            col = 0;
        }
    }

    // Cursor at the very end of the text (one past the last char).
    if !cursor_placed {
        cur_row = rows.len();
        cur_col = col;
    }

    // Flush the final partial row (or guarantee at least one empty row).
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }

    // If the cursor landed one-past-the-end exactly on a wrap boundary (the
    // text filled the last row completely), it points at a phantom row beyond
    // the last real one. Add a trailing empty row so the cursor has a real row
    // to sit on and never indexes out of range.
    if cur_row >= rows.len() {
        rows.push(String::new());
    }

    (rows, cur_row, cur_col)
}

/// Render the bordered, auto-expanding input box.
///
/// `width` is the total terminal width to draw within (clamped to a sane
/// minimum so the content area never disappears). The content area is
/// `width - 4` columns wide (`│`, space … space, `│`). The box shows at least
/// [`MIN_ROWS`] content rows, grows one row per wrapped line, caps at
/// [`MAX_ROWS`], and scrolls to keep the cursor's row visible.
pub fn render_box(text: &str, cursor: usize, width: usize) -> RenderedBox {
    let width = width.max(MIN_WIDTH);
    // 2 border columns + 2 padding spaces.
    let content_width = width.saturating_sub(4).max(1);

    let (mut rows, mut cur_row, cur_col) = wrap_with_cursor(text, cursor, content_width);

    // Auto-expand: never fewer than MIN_ROWS content rows.
    while rows.len() < MIN_ROWS {
        rows.push(String::new());
    }

    // Cap the visible window at MAX_ROWS, scrolling so the cursor stays in view.
    let mut start = 0usize;
    if rows.len() > MAX_ROWS {
        if cur_row >= MAX_ROWS {
            start = cur_row - (MAX_ROWS - 1);
        }
        let end = (start + MAX_ROWS).min(rows.len());
        rows = rows[start..end].to_vec();
        cur_row -= start;
    }

    let dashes = "─".repeat(content_width + 2);
    let mut lines = Vec::with_capacity(rows.len() + 2);
    lines.push(format!("┌{dashes}┐"));
    for r in &rows {
        let pad = content_width.saturating_sub(r.chars().count());
        lines.push(format!("│ {}{} │", r, " ".repeat(pad)));
    }
    lines.push(format!("└{dashes}┘"));

    RenderedBox {
        lines,
        // +1 for the top border row.
        cursor_row: cur_row + 1,
        // +2 for the leading "│ " prefix.
        cursor_col: cur_col + 2,
    }
}

/// True iff the bordered input box should be used: `BISCUITS_INPUT=box`
/// (case-insensitive) **and** stdout is a TTY. When false the caller keeps the
/// plain `read_line` path untouched.
pub fn box_enabled() -> bool {
    if !io::stdout().is_terminal() {
        return false;
    }
    std::env::var("BISCUITS_INPUT")
        .map(|v| v.trim().eq_ignore_ascii_case("box"))
        .unwrap_or(false)
}

// ----------------------------------------------------------------------------
// Raw-mode terminal reader (thin, not headless-testable).
// ----------------------------------------------------------------------------

/// RAII guard that restores the terminal's original termios settings on *every*
/// exit path — normal return, `?` propagation, or panic — so raw mode can never
/// leak out of [`read_line_box`].
struct RawModeGuard {
    fd: i32,
    original: libc::termios,
}

impl RawModeGuard {
    /// Enter raw mode on `fd`, returning a guard that restores the saved state
    /// on drop. Disables canonical mode and echo (and the usual raw-mode flags)
    /// so we receive each keystroke as it is typed.
    fn enter(fd: i32) -> io::Result<Self> {
        // SAFETY: `original` is fully initialized by `tcgetattr` before use.
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        // Mirror cfmakeraw's input/output/control/local flag clearing.
        raw.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        // Read returns as soon as at least one byte is available.
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, original })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // Best-effort restore; nothing useful to do if it fails.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.original);
        }
    }
}

/// Current terminal width in columns, defaulting to 80 when unknown.
fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .max(MIN_WIDTH)
}

/// Paint the box at the current top-left origin and leave the real cursor at the
/// reported (row, col). `prev_rows` is how many lines the *previous* render
/// occupied and `prev_cursor_row` is where it left the real cursor (its row
/// within that box, 0-based among `lines`). On the very first paint pass
/// `prev_rows == 0`.
///
/// Returns `(new_rows, new_cursor_row)` to feed back into the next call. The
/// repaint never scrolls the screen: it re-anchors to the top of the previous
/// box, clears each old line, prints the fresh box, clears any leftover trailing
/// rows, then positions the cursor.
fn paint<W: Write>(
    out: &mut W,
    editor: &InputEditor,
    prev_rows: usize,
    prev_cursor_row: usize,
) -> io::Result<(usize, usize)> {
    let rb = render_box(editor.text(), editor.cursor(), term_width());
    let new_rows = rb.lines.len();

    // Re-anchor to the top-left of the previous box: the real cursor currently
    // sits on `prev_cursor_row`, so move up that many rows and to column 0.
    write!(out, "\r")?;
    if prev_cursor_row > 0 {
        write!(out, "\x1b[{prev_cursor_row}A")?;
    }

    // Repaint each box line, clearing it first so a shorter line can't leave
    // stale glyphs behind.
    for (i, line) in rb.lines.iter().enumerate() {
        write!(out, "\x1b[2K{line}\r")?;
        if i + 1 < new_rows {
            writeln!(out)?;
        }
    }

    // If the previous box was taller, clear the now-unused trailing rows.
    if prev_rows > new_rows {
        for _ in new_rows..prev_rows {
            write!(out, "\n\x1b[2K\r")?;
        }
        // Move back up to the bottom of the new box.
        let extra = prev_rows - new_rows;
        write!(out, "\x1b[{extra}A")?;
    }

    // The cursor is now at the bottom line of the new box (row new_rows-1, col 0).
    // Move up to the cursor's row and right to its column.
    let up = (new_rows.saturating_sub(1)).saturating_sub(rb.cursor_row);
    if up > 0 {
        write!(out, "\x1b[{up}A")?;
    }
    if rb.cursor_col > 0 {
        write!(out, "\x1b[{}C", rb.cursor_col)?;
    }
    out.flush()?;
    Ok((new_rows, rb.cursor_row))
}

/// Read one line of input through the bordered box.
///
/// Returns:
///  * `Ok(Some(text))` on Enter (text may be empty — the caller handles that),
///  * `Ok(None)` when this path should be skipped or aborted: not a TTY,
///    Ctrl-D on an empty buffer (EOF), or Ctrl-C (the outer loop's own Ctrl-C
///    handler then takes over).
///
/// On `Ok(None)` for the not-a-TTY case the caller falls back to plain
/// `read_line`; for Ctrl-C/Ctrl-D it treats it like interrupt/EOF.
///
/// v1 limitation: this renders in the normal cursor flow and does **not**
/// special-case the status-bar DECSTBM scroll region beyond not crashing. With
/// the status bar active the box draws within the scroll region like any other
/// output, which is acceptable for now.
pub fn read_line_box(prompt_hint: &str) -> io::Result<Option<String>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    if !stdin.is_terminal() || !stdout.is_terminal() {
        return Ok(None);
    }

    // Raw mode for stdin; the guard restores it on every exit path (incl. `?`).
    let _guard = RawModeGuard::enter(libc::STDIN_FILENO)?;

    let mut editor = InputEditor::new();
    let mut out = stdout.lock();
    let mut input = stdin.lock();

    // Print the hint above the box once (e.g. the styled prompt glyph), then
    // draw the initial box.
    if !prompt_hint.is_empty() {
        write!(out, "{prompt_hint}\r\n")?;
    }
    let (mut prev_rows, mut prev_cur) = paint(&mut out, &editor, 0, 0)?;

    let mut byte_buf: Vec<u8> = Vec::new();
    let mut one = [0u8; 1];

    macro_rules! repaint {
        () => {{
            let (r, c) = paint(&mut out, &editor, prev_rows, prev_cur)?;
            prev_rows = r;
            prev_cur = c;
        }};
    }

    loop {
        let n = input.read(&mut one)?;
        if n == 0 {
            // stdin closed.
            finish(&mut out, prev_rows, prev_cur)?;
            return Ok(None);
        }
        let b = one[0];

        match b {
            0x03 => {
                // Ctrl-C: bail; the outer loop's Ctrl-C handler takes over.
                finish(&mut out, prev_rows, prev_cur)?;
                return Ok(None);
            }
            0x04 => {
                // Ctrl-D: EOF only when the buffer is empty.
                if editor.is_empty() {
                    finish(&mut out, prev_rows, prev_cur)?;
                    return Ok(None);
                }
            }
            0x17 => {
                // Ctrl-W: delete previous word.
                editor.delete_word();
                repaint!();
            }
            b'\r' | b'\n' => {
                // Submit.
                finish(&mut out, prev_rows, prev_cur)?;
                return Ok(Some(editor.text().to_string()));
            }
            0x7f | 0x08 => {
                // Backspace / Ctrl-H.
                editor.backspace();
                repaint!();
            }
            0x1b => {
                // Escape sequence: arrows, Home/End, Delete.
                handle_escape(&mut input, &mut editor)?;
                repaint!();
            }
            _ => {
                // Accumulate (possibly multi-byte UTF-8) printable input. Skip
                // other C0 control chars we don't handle.
                if b < 0x20 {
                    continue;
                }
                byte_buf.clear();
                byte_buf.push(b);
                let needed = utf8_continuation_bytes(b);
                for _ in 0..needed {
                    if input.read(&mut one)? == 0 {
                        break;
                    }
                    byte_buf.push(one[0]);
                }
                if let Ok(s) = std::str::from_utf8(&byte_buf) {
                    editor.insert_str(s);
                    repaint!();
                }
                // Invalid UTF-8 is silently dropped.
            }
        }
    }
}

/// Number of UTF-8 continuation bytes that follow a given lead byte.
fn utf8_continuation_bytes(lead: u8) -> usize {
    if lead >= 0xf0 {
        3
    } else if lead >= 0xe0 {
        2
    } else if lead >= 0xc0 {
        1
    } else {
        0
    }
}

/// Read and apply an escape sequence (the ESC byte was already consumed).
/// Recognizes arrows (`[C`/`[D`), Home/End (`[H`/`[F`, `[1~`/`[4~`), and Delete
/// (`[3~`). Unrecognized sequences are consumed and ignored.
fn handle_escape<R: Read>(input: &mut R, editor: &mut InputEditor) -> io::Result<()> {
    let mut b = [0u8; 1];
    if input.read(&mut b)? == 0 {
        return Ok(());
    }
    if b[0] != b'[' && b[0] != b'O' {
        return Ok(());
    }
    if input.read(&mut b)? == 0 {
        return Ok(());
    }
    match b[0] {
        b'C' => editor.move_right(),
        b'D' => editor.move_left(),
        b'H' => editor.move_home(),
        b'F' => editor.move_end(),
        c @ b'0'..=b'9' => {
            // Numeric/parameterized sequence like `[3~`, `[1~`, `[4~`, or a
            // modified arrow such as `[1;5C`. Read the parameter bytes until the
            // CSI final byte (any byte in 0x40..=0x7e, e.g. `~`, `C`, `R`), then
            // stop. BUG FIX: previously this only stopped on `~`, so a sequence
            // ending in a letter (e.g. Ctrl-Right `[1;5C`) was never terminated
            // and the loop blocked, swallowing the user's next keystroke. We now
            // stop on the proper terminator and only act on the recognized `~`
            // forms; any other final byte is consumed and ignored.
            let mut params = vec![c];
            let mut terminator = 0u8;
            loop {
                if input.read(&mut b)? == 0 {
                    break;
                }
                if (0x40..=0x7e).contains(&b[0]) {
                    terminator = b[0];
                    break;
                }
                params.push(b[0]);
            }
            if terminator == b'~' {
                match params.as_slice() {
                    [b'1'] => editor.move_home(),
                    [b'4'] => editor.move_end(),
                    [b'3'] => editor.delete(),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Finish: move below the box and leave the terminal on a fresh line so the
/// agent's output begins cleanly underneath. The real cursor is currently on
/// `cursor_row` within a box of `total_rows` lines.
fn finish<W: Write>(out: &mut W, total_rows: usize, cursor_row: usize) -> io::Result<()> {
    let down = total_rows.saturating_sub(cursor_row);
    write!(out, "\r")?;
    for _ in 0..down {
        writeln!(out)?;
    }
    write!(out, "\r")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Editing: insert / cursor basics ----

    #[test]
    fn insert_appends_and_advances_cursor() {
        let mut e = InputEditor::new();
        for c in "abc".chars() {
            e.insert_char(c);
        }
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), 3);
        assert!(!e.is_empty());
    }

    #[test]
    fn new_editor_is_empty() {
        let e = InputEditor::new();
        assert!(e.is_empty());
        assert_eq!(e.cursor(), 0);
        assert_eq!(e.text(), "");
    }

    #[test]
    fn insert_in_middle() {
        let mut e = InputEditor::new();
        e.insert_str("ac");
        e.move_left(); // cursor between a and c
        e.insert_char('b');
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), 2);
    }

    #[test]
    fn insert_at_start() {
        let mut e = InputEditor::new();
        e.insert_str("bc");
        e.move_home();
        e.insert_char('a');
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), 1);
    }

    // ---- Backspace ----

    #[test]
    fn backspace_at_end() {
        let mut e = InputEditor::new();
        e.insert_str("abc");
        e.backspace();
        assert_eq!(e.text(), "ab");
        assert_eq!(e.cursor(), 2);
    }

    #[test]
    fn backspace_in_middle() {
        let mut e = InputEditor::new();
        e.insert_str("abc");
        e.move_left(); // between b and c
        e.backspace(); // deletes b
        assert_eq!(e.text(), "ac");
        assert_eq!(e.cursor(), 1);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut e = InputEditor::new();
        e.insert_str("abc");
        e.move_home();
        e.backspace();
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), 0);
    }

    // ---- Delete ----

    #[test]
    fn delete_at_cursor() {
        let mut e = InputEditor::new();
        e.insert_str("abc");
        e.move_home();
        e.delete(); // removes a
        assert_eq!(e.text(), "bc");
        assert_eq!(e.cursor(), 0);
    }

    #[test]
    fn delete_at_end_is_noop() {
        let mut e = InputEditor::new();
        e.insert_str("abc");
        e.delete();
        assert_eq!(e.text(), "abc");
        assert_eq!(e.cursor(), 3);
    }

    #[test]
    fn delete_in_middle() {
        let mut e = InputEditor::new();
        e.insert_str("abc");
        e.move_left();
        e.move_left(); // between a and b
        e.delete(); // removes b
        assert_eq!(e.text(), "ac");
        assert_eq!(e.cursor(), 1);
    }

    // ---- Movement clamps ----

    #[test]
    fn move_left_clamps_at_start() {
        let mut e = InputEditor::new();
        e.insert_str("ab");
        e.move_home();
        e.move_left();
        assert_eq!(e.cursor(), 0);
    }

    #[test]
    fn move_right_clamps_at_end() {
        let mut e = InputEditor::new();
        e.insert_str("ab");
        e.move_right();
        assert_eq!(e.cursor(), 2);
    }

    #[test]
    fn home_and_end() {
        let mut e = InputEditor::new();
        e.insert_str("hello");
        e.move_home();
        assert_eq!(e.cursor(), 0);
        e.move_end();
        assert_eq!(e.cursor(), 5);
    }

    #[test]
    fn clear_resets() {
        let mut e = InputEditor::new();
        e.insert_str("hello");
        e.clear();
        assert!(e.is_empty());
        assert_eq!(e.cursor(), 0);
    }

    // ---- UTF-8 correctness (must never panic on char boundaries) ----

    #[test]
    fn utf8_accented_backspace_no_panic() {
        let mut e = InputEditor::new();
        e.insert_str("héllo"); // 'é' is 2 bytes
        assert_eq!(e.cursor(), 5);
        e.backspace(); // remove 'o'
        e.backspace(); // remove 'l'
        e.backspace(); // remove 'l'
        e.backspace(); // remove 'é'  <-- byte-naive code would panic here
        assert_eq!(e.text(), "h");
        assert_eq!(e.cursor(), 1);
    }

    #[test]
    fn utf8_insert_before_multibyte() {
        let mut e = InputEditor::new();
        e.insert_str("é"); // 2 bytes, 1 char
        e.move_home();
        e.insert_char('x');
        assert_eq!(e.text(), "xé");
        assert_eq!(e.cursor(), 1);
    }

    #[test]
    fn emoji_editing_no_panic() {
        let mut e = InputEditor::new();
        e.insert_str("a😀b"); // emoji is 4 bytes, 1 char
        assert_eq!(e.cursor(), 3);
        e.move_left(); // between 😀 and b
        e.backspace(); // delete the emoji
        assert_eq!(e.text(), "ab");
        assert_eq!(e.cursor(), 1);
    }

    #[test]
    fn delete_at_multibyte_cursor() {
        let mut e = InputEditor::new();
        e.insert_str("é😀");
        e.move_home();
        e.delete(); // delete é
        assert_eq!(e.text(), "😀");
        e.delete(); // delete emoji
        assert_eq!(e.text(), "");
    }

    // ---- delete_word (Ctrl-W) ----

    #[test]
    fn delete_word_removes_last_word() {
        let mut e = InputEditor::new();
        e.insert_str("hello world");
        e.delete_word();
        assert_eq!(e.text(), "hello ");
        assert_eq!(e.cursor(), 6);
    }

    #[test]
    fn delete_word_skips_trailing_space() {
        let mut e = InputEditor::new();
        e.insert_str("hello world   ");
        e.delete_word();
        assert_eq!(e.text(), "hello ");
    }

    #[test]
    fn delete_word_at_start_noop() {
        let mut e = InputEditor::new();
        e.insert_str("hello");
        e.move_home();
        e.delete_word();
        assert_eq!(e.text(), "hello");
    }

    #[test]
    fn delete_word_in_middle() {
        let mut e = InputEditor::new();
        e.insert_str("foo bar baz");
        // cursor at end of "bar" -> move left 4 (past " baz")
        for _ in 0..4 {
            e.move_left();
        }
        e.delete_word(); // removes "bar"
        assert_eq!(e.text(), "foo  baz");
    }

    #[test]
    fn delete_word_only_spaces() {
        let mut e = InputEditor::new();
        e.insert_str("    ");
        e.delete_word();
        assert_eq!(e.text(), "");
        assert_eq!(e.cursor(), 0);
    }

    // ---- render_box: borders, padding, content width ----

    fn content_rows(rb: &RenderedBox) -> &[String] {
        &rb.lines[1..rb.lines.len() - 1]
    }

    #[test]
    fn render_box_has_borders_and_min_two_rows() {
        let rb = render_box("hi", 2, 20);
        assert!(rb.lines.first().unwrap().starts_with('┌'));
        assert!(rb.lines.first().unwrap().ends_with('┐'));
        assert!(rb.lines.last().unwrap().starts_with('└'));
        assert!(rb.lines.last().unwrap().ends_with('┘'));
        // 2 content rows minimum + 2 borders.
        assert_eq!(rb.lines.len(), MIN_ROWS + 2);
        for row in content_rows(&rb) {
            assert!(row.starts_with('│'));
            assert!(row.ends_with('│'));
        }
    }

    #[test]
    fn render_box_content_width_is_width_minus_four() {
        let width = 20;
        let rb = render_box("", 0, width);
        // Each content row should be exactly `width` display chars wide:
        // │ + space + content_width + space + │
        for row in content_rows(&rb) {
            assert_eq!(row.chars().count(), width);
        }
        // Top border is also `width` wide.
        assert_eq!(rb.lines[0].chars().count(), width);
    }

    #[test]
    fn render_box_pads_short_content() {
        let rb = render_box("ab", 2, 12); // content_width = 8
        let first = &content_rows(&rb)[0];
        // "│ ab      │" — 8-char content area with "ab" + 6 spaces.
        assert_eq!(first, "│ ab       │");
        assert_eq!(first.chars().count(), 12);
    }

    // ---- render_box: wrapping ----

    #[test]
    fn render_box_wraps_at_content_width() {
        // width 9 => content_width = 5. "abcdefg" (7 chars) wraps to 2 rows.
        let rb = render_box("abcdefg", 7, 9);
        let rows = content_rows(&rb);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains("abcde"));
        assert!(rows[1].contains("fg"));
    }

    #[test]
    fn render_box_wraps_exactly_at_boundary() {
        // content_width = 5, text exactly 5 chars: should stay 1 wrapped row,
        // but min 2 content rows means the box still has 2 rows.
        let rb = render_box("abcde", 5, 9);
        let rows = content_rows(&rb);
        assert_eq!(rows.len(), 2); // min rows
        assert!(rows[0].contains("abcde"));
        // Cursor sat one past the 5th char -> should be on row 2 (next line).
        assert_eq!(rb.cursor_row, 2);
        assert_eq!(rb.cursor_col, 2); // start of content on the second row
    }

    #[test]
    fn render_box_auto_expands_past_min() {
        // content_width = 5; 13 chars -> 3 wrapped rows (5+5+3).
        let rb = render_box("abcdefghijklm", 13, 9);
        let rows = content_rows(&rb);
        assert_eq!(rows.len(), 3);
    }

    // ---- render_box: cursor (row, col) ----

    #[test]
    fn cursor_at_start() {
        let rb = render_box("hello", 0, 20);
        assert_eq!(rb.cursor_row, 1); // first content row
        assert_eq!(rb.cursor_col, 2); // just after "│ "
    }

    #[test]
    fn cursor_in_middle_no_wrap() {
        let rb = render_box("hello", 3, 20);
        assert_eq!(rb.cursor_row, 1);
        assert_eq!(rb.cursor_col, 2 + 3);
    }

    #[test]
    fn cursor_at_end_no_wrap() {
        let rb = render_box("hello", 5, 20);
        assert_eq!(rb.cursor_row, 1);
        assert_eq!(rb.cursor_col, 2 + 5);
    }

    #[test]
    fn cursor_after_wrap() {
        // content_width = 5; "abcdefg", cursor at index 6 (on 'g', second row).
        let rb = render_box("abcdefg", 6, 9);
        assert_eq!(rb.cursor_row, 2); // second content row
        assert_eq!(rb.cursor_col, 2 + 1); // 'g' is the 2nd char of row 2
    }

    #[test]
    fn cursor_at_very_end_after_wrap() {
        // "abcdefg" 7 chars, content_width 5, cursor past last char.
        let rb = render_box("abcdefg", 7, 9);
        assert_eq!(rb.cursor_row, 2);
        assert_eq!(rb.cursor_col, 2 + 2); // after "fg"
    }

    // ---- render_box: scrolling cap at MAX_ROWS ----

    #[test]
    fn render_box_caps_rows_and_keeps_cursor_visible() {
        // content_width = 5; build text that wraps into > MAX_ROWS rows.
        let text: String = "x".repeat(5 * (MAX_ROWS + 5));
        let cursor = text.chars().count(); // at the very end
        let rb = render_box(&text, cursor, 9);
        let rows = content_rows(&rb);
        assert_eq!(rows.len(), MAX_ROWS); // capped
                                          // Cursor must still be within the visible window.
        assert!(rb.cursor_row >= 1 && rb.cursor_row <= MAX_ROWS);
    }

    // ---- render_box: degenerate widths must not panic ----

    #[test]
    fn render_box_zero_width_no_panic() {
        let rb = render_box("hello", 5, 0);
        // Clamped to MIN_WIDTH; should still produce a valid box.
        assert!(rb.lines.len() >= MIN_ROWS + 2);
        for line in &rb.lines {
            assert!(!line.is_empty());
        }
    }

    #[test]
    fn render_box_tiny_width_no_panic() {
        for w in 0..6 {
            let rb = render_box("abcdef", 6, w);
            assert!(rb.lines.len() >= MIN_ROWS + 2);
        }
    }

    #[test]
    fn render_box_empty_text() {
        let rb = render_box("", 0, 10);
        let rows = content_rows(&rb);
        assert_eq!(rows.len(), MIN_ROWS);
        assert_eq!(rb.cursor_row, 1);
        assert_eq!(rb.cursor_col, 2);
    }

    // ---- render_box: UTF-8 content padding correctness ----

    #[test]
    fn render_box_utf8_content_no_panic() {
        let rb = render_box("héllo😀", 6, 20);
        // Just ensure it renders and the cursor is placed by char count.
        assert_eq!(rb.cursor_row, 1);
        assert_eq!(rb.cursor_col, 2 + 6);
    }

    // ---- helpers ----

    #[test]
    fn utf8_continuation_byte_counts() {
        assert_eq!(utf8_continuation_bytes(b'a'), 0); // ASCII
        assert_eq!(utf8_continuation_bytes(0xc3), 1); // 2-byte lead
        assert_eq!(utf8_continuation_bytes(0xe2), 2); // 3-byte lead
        assert_eq!(utf8_continuation_bytes(0xf0), 3); // 4-byte lead
    }

    // ---- handle_escape: byte consumption ----
    //
    // The ESC byte is consumed by the caller, so each fixture below is the bytes
    // that follow ESC. We feed a trailing sentinel byte and assert it is NOT
    // consumed, proving the escape parser stops at the sequence's true end and
    // never eats the user's next keystroke.

    #[test]
    fn escape_arrow_right_moves_and_stops() {
        let mut e = InputEditor::new();
        e.insert_str("ab");
        e.move_home(); // cursor at 0
        let mut seq: &[u8] = b"[Cx"; // arrow-right, then sentinel 'x'
        handle_escape(&mut seq, &mut e).unwrap();
        assert_eq!(e.cursor(), 1); // moved right
        assert_eq!(seq, b"x"); // sentinel untouched
    }

    #[test]
    fn escape_delete_tilde_acts_and_stops() {
        let mut e = InputEditor::new();
        e.insert_str("abc");
        e.move_home();
        let mut seq: &[u8] = b"[3~Z"; // Delete, then sentinel
        handle_escape(&mut seq, &mut e).unwrap();
        assert_eq!(e.text(), "bc"); // 'a' deleted
        assert_eq!(seq, b"Z"); // sentinel untouched
    }

    #[test]
    fn escape_modified_arrow_does_not_swallow_next_key() {
        // REGRESSION: `[1;5C` (Ctrl-Right) ends in a letter, not `~`. The old
        // parser looped waiting for `~` and consumed the following keystroke. It
        // must now stop at `C` and leave the sentinel byte for the reader.
        let mut e = InputEditor::new();
        e.insert_str("hello");
        e.move_home();
        let mut seq: &[u8] = b"[1;5CQ"; // modified arrow, then sentinel 'Q'
        handle_escape(&mut seq, &mut e).unwrap();
        // Unrecognized (non-`~`) sequence: cursor unchanged.
        assert_eq!(e.cursor(), 0);
        // Crucially, the sentinel must remain unread.
        assert_eq!(seq, b"Q");
    }

    #[test]
    fn escape_home_end_numeric_forms() {
        let mut e = InputEditor::new();
        e.insert_str("hello");
        // `[1~` = Home
        let mut seq: &[u8] = b"[1~";
        handle_escape(&mut seq, &mut e).unwrap();
        assert_eq!(e.cursor(), 0);
        // `[4~` = End
        let mut seq: &[u8] = b"[4~";
        handle_escape(&mut seq, &mut e).unwrap();
        assert_eq!(e.cursor(), 5);
    }
}
