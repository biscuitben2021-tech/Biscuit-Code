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

- `.biscuits/` for conversations, settings, goals, eval reports, MCP config, skill state, and runtime state
- `BISCUITS.md` for editable project memory
- `biscuit/handoff.md` for current project requirements and handoff notes
- `biscuit/logs.md` for runtime-maintained change logs
- `skills/` (optional) for shared skill packs you commit alongside the project

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
/skills                list discovered skills and their status
/skills disable <name> stop a skill from being injected
```

## Skills

Skills are portable Markdown instruction packs that teach Biscuits a reusable
workflow, tool preference, or domain behavior. Each skill lives in its own
folder containing a `SKILL.md` file. Biscuits discovers skills at startup,
then for every message it selects only the few skills that look relevant and
injects them into the model context. Skills that do not match are never sent,
so prompts stay small.

### Folder layout

Biscuits looks for skills in three places, highest precedence first. When the
same skill name appears in more than one place, the higher-precedence copy
wins:

```text
.biscuits/skills/<name>/SKILL.md   # project skills (per workspace, not committed)
skills/<name>/SKILL.md             # shared repo skills (commit to share with your team)
<config>/biscuits/skills/<name>/SKILL.md   # your personal global skills
```

The global directory follows your OS config location:

- macOS: `~/Library/Application Support/biscuits/skills/`
- Linux: `~/.config/biscuits/skills/`
- Windows: `%APPDATA%\biscuits\skills\`

### Example `SKILL.md`

```markdown
---
name: rust-debugger
description: Debugs Rust compiler, clippy, test, and CI failures.
triggers:
  - rust
  - cargo
  - clippy
  - compiler error
tools:
  - Read
  - Grep
  - Bash
  - Edit
enabled: true
---

# Rust Debugger

When a Rust build or test fails:

1. Read the failing command output and find the first real error.
2. Open the referenced file and locate the root cause.
3. Make the smallest correct fix, then re-run the failing command.
```

The frontmatter is optional. If it is missing, Biscuits infers the skill name
from the folder name and uses the first heading or paragraph as the
description. `triggers`, `tools`, and `enabled` are all optional too.

### How selection works

For each message, Biscuits scores enabled skills by matching trigger phrases,
the skill name, and description keywords, preferring strong matches (a trigger
or name hit) over weak keyword overlap. Ask about a Rust failure and the
`rust-debugger` skill above is selected automatically. Skills are treated as
guidance, not absolute truth — your latest instructions always override them.

### Commands

```text
/skills                    list discovered skills with enabled/disabled status and source
/skills refresh            reload skills from disk
/skills show <name>        show a skill's metadata and file path
/skills enable <name>      enable a skill
/skills disable <name>     disable a skill
/skills selected <message> debug which skills a message would select
```

Enable/disable state is stored in `.biscuits/skills.json`; the `SKILL.md`
source files are never modified. Skills work the same on macOS, Windows, and
Linux.

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

## Biscuit Browser (experimental, separate app)

`biscuit-browser/` contains **Biscuit Browser** — an AI-native Chromium browser
(Electron + React + TypeScript) for macOS and Windows. It is a self-contained
app added alongside this Rust CLI "like a plugin"; it does not modify the CLI.
It has tabs/address bar/settings plus an AI side panel, and an agent that
operates pages via a structured **Agent View** (`@e` refs, not raw HTML),
gated by a per-task contract and permission modes.

```sh
cd biscuit-browser
npm install
npm run dev
```

It is built to connect to this Rust `biscuits` CLI later as a model/agent
backend. See [`biscuit-browser/README.md`](biscuit-browser/README.md).
