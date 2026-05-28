# Biscuits

Biscuits is a lightweight terminal AI agent that works from whatever folder you launch it in. Point it at a project, connect an API key, choose a model, and it can help with planning, editing, command execution, web lookup, local memory, project handoff notes, evals, MCP tools, and more than just coding.

It is built to be customizable without being heavy: swap providers, use local or hosted models, tune memory and privacy, add shortcuts, connect MCP servers, and keep per-folder project knowledge in plain files.

## Install

Biscuits is a Rust CLI. Install Rust with `rustup` first.

### macOS

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

From this repo checkout:

```sh
cargo install --path .
biscuits
```

The singular alias works too:

```sh
biscuit
```

### Windows

Install Rust from [rustup.rs](https://rustup.rs/) or [rust-lang.org/tools/install](https://www.rust-lang.org/tools/install). When prompted, install the Visual Studio C++ build tools.

Open a new PowerShell window from this repo checkout:

```powershell
cargo install --path .
biscuits
```

If PowerShell cannot find `biscuits`, make sure this is on your `Path`:

```powershell
%USERPROFILE%\.cargo\bin
```

The singular alias works on Windows too:

```powershell
biscuit
```

## Start In Any Folder

```sh
cd path/to/your/project
biscuits
```

On first launch, choose a provider and model. Biscuits supports:

- OpenAI
- Anthropic
- Google Gemini
- OpenAI-compatible APIs
- LM Studio

API keys are requested at launch or read from environment variables. The saved profile stores provider, model, base URL, and optional custom prompt, not the API key.

## Workspace Files

Biscuits keeps project-local state in the folder you launch it from:

- `.biscuits/` for conversations, settings, goals, eval reports, MCP config, and runtime state
- `BISCUITS.md` for editable project memory
- `biscuit/handoff.md` for current project requirements and handoff notes
- `biscuit/logs.md` for runtime-maintained change logs

These runtime files are ignored by default except `BISCUITS.md`, which you may choose to commit when project memory should travel with the repo.

## Useful Commands

```text
/help                  show commands
/clear                 start a new chat while preserving saved memory
/remember <fact>       save a durable memory
/forget <phrase>       forget matching memories
/memories              inspect editable user memory
/biscuits              inspect project memory
/handoff               inspect project handoff notes
/sessions              list saved sessions
/resume <id>           resume a saved session
/config                show saved provider/model profile
/config prompt <text>  set a custom system prompt
/shortcut add <k> <c>  add a local shortcut
/permissions           inspect permission mode
/permissions auto      let normal workspace actions run without prompting
/privacy incognito     avoid saving, memorizing, or retrieving memories
/memory-mode best      extract memory after every normal turn
/memory-mode hybrid    extract memory every third normal turn
/memory-mode tool      only save explicit /remember memories
/mcp                   connect and use MCP servers
```

## Development

```sh
cargo fmt --check
cargo test
cargo build --release
biscuits eval --smoke
```

The test harness can snapshot and compare project test results:

```sh
biscuits harness baseline
biscuits harness run
biscuits harness diff
```

## Platform Notes

Core chat, file tools, shell commands, memory, evals, and MCP are designed for macOS, Windows, and Linux. GUI computer-use helpers are more limited: screenshots are implemented for macOS and Linux, while mouse/keyboard control is currently implemented for macOS.
