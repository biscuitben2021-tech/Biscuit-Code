---
name: rust-debugger
description: Debugs Rust compiler, clippy, test, and CI failures.
triggers:
  - rust
  - cargo
  - clippy
  - compiler error
  - test failure
  - ci failure
tools:
  - Read
  - Grep
  - Bash
  - Edit
enabled: true
---

# Rust Debugger

Use this when a Rust build, lint, test, or CI run is failing.

## Approach

1. Reproduce the failure. Run the exact failing command (for example
   `cargo build`, `cargo clippy --all-targets -- -D warnings`, or
   `cargo test --locked`) so you see the real output, not a guess.
2. Read the first error, not the last. Later errors are often cascades from
   the first one. Note the file, line, and error code (for example `E0382`).
3. Open the referenced file with Read and study the surrounding code before
   changing anything.
4. Fix the root cause with the smallest correct change. Prefer `Edit` for
   precise edits. Do not silence errors with `#[allow(...)]` or `unwrap()`
   unless that is genuinely the right call.
5. Re-run the failing command to confirm it now passes, then run the wider
   check suite so the fix did not break anything else.

## Notes

- Borrow-checker errors usually mean a lifetime, ownership, or clone decision
  is wrong — rethink the data flow rather than scattering `.clone()`.
- For clippy lints, read the lint name and its suggested fix; apply the
  idiomatic form instead of `#[allow]`.
- Keep changes consistent with the surrounding code's style and error handling.
