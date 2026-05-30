# 🍪 Biscuit Browser

**An AI-native browser where the agent reads the page like an accessibility tree — not raw HTML or screenshots — and every action it takes passes a permission gate first.**

[![CI](https://github.com/biscuitben2021-tech/Biscuit-Code/actions/workflows/ci.yml/badge.svg)](https://github.com/biscuitben2021-tech/Biscuit-Code/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
&nbsp;Electron · React · TypeScript · macOS / Windows / Linux

Biscuit Browser is a normal Chromium browser — tabs, an address bar,
back/forward/reload — plus an AI side panel. You give it a task in plain
English; it works the page through a structured **Agent View** (stable `@e`
element refs, accessible labels, roles, state) instead of guessing from a
screenshot. Before it clicks, types, or navigates, each step is checked against
a **Task Contract** derived from *your prompt* and a **permissioned Action
Gate**. You stay in control: approve, deny, or hit Stop.

> **Status: V1 / experimental.** It runs and is useful for read-mostly tasks
> (research, lookups, navigating docs). The safety layer is unit-tested in CI.
> See [Limitations](#limitations-read-this) for what it does *not* do yet — we'd
> rather under-promise.

## Demo

![Biscuit Browser demo](docs/demo.gif)

> The demo GIF and screenshots are captured from the running app — see
> [`docs/`](docs/README.md) for exactly what to record and where it goes. No API
> key? Launch and use the built-in **Demo mode** (no model required) to see the
> Agent View, a Task Contract, and the Action Gate in action first.

### Screenshots

Pending capture (see [`docs/README.md`](docs/README.md)): the normal browser +
side panel, the Agent View (`@e` refs + coverage notes), the Task Contract, an
approval prompt, the Bypass (Danger) banner, and the Action Log.

---

## Why you might care

Most "browser agents" either screenshot the page and hope a vision model clicks
the right pixel, or dump raw HTML into the prompt. Both are brittle and neither
gives you a say before the agent does something irreversible. Biscuit's bet is:

- **A structured Agent View beats pixels and raw HTML.** The agent sees a
  compact list of interactive elements with stable refs (`@e1`, `@e2`…), roles,
  labels, and state — closer to how a screen reader sees a page. No raw HTML, no
  screenshots by default.
- **Intent should be locked before action.** A Task Contract is generated from
  *your words only* (never page content) and, once locked, is immutable for that
  task. A malicious page can't talk the agent into new powers.
- **Every action is gated.** Navigations, clicks, and typing are classified by
  risk and page context, then allowed / asked / blocked per your permission
  mode. Money, logins, deletions, and "Confirm" buttons on checkout pages are
  treated as dangerous by default.

## Run it in ~2 minutes

You need **Node.js 18+** (20 LTS recommended) and npm.

```sh
git clone https://github.com/biscuitben2021-tech/Biscuit-Code.git
cd Biscuit-Code/biscuit-browser
npm install      # downloads Electron's Chromium on first run
npm run dev      # launches the app with hot reload
```

On Windows use PowerShell; on Linux the same commands work. First launch opens a
tab — open **Settings** in the right panel to pick a provider/model and paste an
API key. No key? Point it at a local **LM Studio** / OpenAI-compatible server.

Then type a task in **Chat**, e.g.
*"Find the current stable Rust version from the official Rust website."*

## What it looks like

The window is a normal browser on the left and an AI panel on the right:

```
┌───────────────────────────────────────────────┬───────────────────────┐
│  ◀ ▶ ⟳   [ address bar            ]  [assisted▾] ■Stop                  │
├───────────────────────────────────────────────┤  Chat | Contract |    │
│                                                 │  Agent View | Log |   │
│                                                 │  Approvals | Settings │
│            the live web page                    ├───────────────────────┤
│         (real Chromium WebContentsView)         │  > Find the stable    │
│                                                 │    Rust version…      │
│                                                 │  [assistant] opened … │
│                                                 │  [awaiting-approval]  │
└───────────────────────────────────────────────┴───────────────────────┘
```

- **Chat** — give the task; watch progress.
- **Contract** — the generated Task Contract; review/edit/lock it (always
  required in Safe mode).
- **Agent View** — exactly what the agent sees: text + `@e` refs, plus an honest
  coverage report (frames/shadow roots it could and couldn't reach). No raw HTML.
- **Action Log** — every gate decision and action, including in Bypass.
- **Approvals** — pending actions the gate flagged. Approve / Deny.
- **Settings** — provider, model, base URL, API key, default mode, expert mode.

## Permission modes

| Mode               | Behavior                                                              |
| ------------------ | --------------------------------------------------------------------- |
| **Safe**           | You review & lock the contract; the gate asks before most actions.    |
| **Assisted**       | *(default)* Low/medium-risk actions run; high-risk ones ask first.    |
| **Auto**           | Acts inside the contract; only high-risk actions ask.                 |
| **Bypass (Danger)** | Expert-only. No prompts, no contract — but Stop + logging stay on.   |

**Bypass is deliberately hard to enable.** It is labeled **Bypass (Danger)**,
hidden unless you turn on *Expert mode* in Settings, requires typing
`ENABLE BYPASS` to arm, shows a persistent red banner while active, cannot be
saved as a default, and is automatically dropped back to Assisted when you press
Stop or disable expert mode.

## Is it safe?

**Honest framing first:** Biscuit *reduces* prompt-injection risk — it does
**not** "prevent" it, and no agent browser can today. It makes the agent's
intent explicit (the locked Task Contract), its view of the page inspectable
(the Agent View), and every action revocable (the Action Gate + Stop). Read the
full [**Threat Model**](docs/THREAT_MODEL.md) for trust boundaries and known
gaps. With that framing, the concrete mechanisms:

- **Locked-down processes.** `contextIsolation`, `sandbox`, and
  `nodeIntegration: false` for both the app UI and every browsed page. Browsed
  pages run in a **separate session with no preload** — they can never reach app
  APIs or your key.
- **One typed bridge.** The renderer talks to main only through the
  `window.biscuit` surface in `preload`. No raw Node, no `ipcRenderer`, no
  arbitrary channels.
- **Keys stay in main.** API keys are encrypted at rest with the OS keychain
  (`safeStorage`), used only for model calls, and never sent to a page or
  returned to the renderer.
- **Untrusted page data.** The Agent View and all page text are delivered to the
  model as clearly-delimited *untrusted data*; the system prompt forbids treating
  page content as instructions. The locked contract is the only authority.
- **Filesystem/script schemes are unreachable.** The address bar and the agent
  can only navigate `http(s)`/`about:blank`; `file:`, `data:`, `javascript:`,
  etc. are routed to a web search instead.
- **Context-aware gating.** The gate inspects the target element *and* the page
  context (URL, title, headings, text). A vague "Continue" on a checkout page is
  treated as a payment action and blocked by the default contract.
- **Verification.** After a mutating action the runtime fingerprints the page
  before/after to tell whether it actually changed and whether it tripped an
  error or form-validation message — instead of confidently continuing on a
  no-op.

Found a vulnerability? See [SECURITY.md](./SECURITY.md).

## Limitations (read this)

We'd rather be honest than oversell:

- **The Agent View is an injected page-world script** (`executeJavaScript`). It
  now traverses **open shadow roots** and **same-origin iframes** recursively,
  and surfaces `cursor:pointer` "JS buttons", covered/disabled elements, and an
  honest coverage report. But it **cannot** see **cross-origin frames**,
  **closed shadow roots**, or **`<canvas>`-painted UIs**, and bounding boxes
  inside iframes are frame-relative. These gaps are reported in the snapshot's
  `context.notes`, not hidden. A **CDP/Playwright controller layer** is the
  planned path for deeper, more robust control (see the roadmap).
- **Verification is heuristic.** It catches navigations, content changes, new
  error/validation banners, and obvious no-ops — not every silent failure.
- **The gate is heuristic and English-centric.** It errs toward asking, but
  novel phrasings or non-English labels may slip a risky action down to "ask"
  instead of "block". The contract and your chosen mode are the backstop.
- **No streaming, no persistence yet.** Chat responses aren't streamed; logs and
  contracts live in memory for the session.

## Testing

The safety-critical logic is pure and unit-tested with **Vitest** (run in CI on
Linux, macOS, and Windows):

```sh
npm run typecheck   # strict tsc for main + preload + renderer
npm test            # vitest: gate, contract, URL, extractor, verify, modes
npm run build       # electron-vite bundle
```

Coverage today: Action Gate decision matrix (all modes), Task Contract
generation/parsing, URL normalization, the Agent View extractor (against jsdom,
incl. shadow DOM + sensitive-field detection), the verification layer, and the
model-output JSON extractor — **69 tests**. Adding a fixture page to the
extractor tests is a great first contribution.

## Architecture

```
src/
  shared/        types.ts, ipc.ts, api.ts, url.ts   (contracts + pure helpers)
  main/          Node/Electron main process (privileged)
    index.ts     app + window boot
    app.ts       orchestrator: mode, contract, approvals, IPC wiring
    tabs.ts      TabManager — a WebContentsView per tab, ref generations
    agent-view/  extract.ts (page-injected extractor), signature.ts (verify)
    actions/     browserActions.ts (click/type/scroll/screenshot), execute.ts
    agent/       taskContract.ts, actionGate.ts, runtime.ts, verify.ts, llm.ts
    settings/    store.ts (encrypted API key via safeStorage)
    log.ts       action/decision log
  preload/       index.ts — contextBridge exposes `window.biscuit` only
  renderer/      React app UI (tabs, toolbar, side panel, settings, modals)
test/            vitest unit tests for the safety layer
```

## Connecting the Rust `biscuits` CLI later

The LLM client (`src/main/agent/llm.ts`) is provider-agnostic and mirrors the
Rust CLI's provider set (OpenAI / Anthropic / Google / OpenAI-compatible /
LM Studio) on purpose. To use the Rust agent as a backend later, add a provider
branch (e.g. `'biscuits-cli'`) that shells out to the `biscuits`/`biscuit`
binary (or talks to a local HTTP endpoint) and returns the same shape. The
runtime, gate, and Agent View are backend-independent. Intentionally **not**
wired up in V1.

## Roadmap

Full roadmap (V1 / V2 / later): [**docs/ROADMAP.md**](docs/ROADMAP.md). Highlights:

- [ ] CDP/Playwright controller for cross-origin frames + robust control (and
      real pointer/keyboard input events) where `executeJavaScript` is insufficient.
- [ ] MutationObserver-based ref invalidation for same-document DOM changes.
- [ ] Streaming model responses into Chat.
- [ ] Persist + export action logs and contracts.
- [ ] Per-action "auto-approve high-risk" config for Auto mode.
- [ ] Code signing / notarization for packaged macOS & Windows builds.
- [ ] Connect the Rust `biscuits` CLI as a model/agent backend.

**Out of scope for V1**: Chrome extensions, cloud browser hosting, a plugin
marketplace, payments, account sync, CAPTCHAs, banking sites.

## Contributing & License

Contributions welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md),
[good first issues](docs/GOOD_FIRST_ISSUES.md), the
[Threat Model](docs/THREAT_MODEL.md), and the [Roadmap](docs/ROADMAP.md).
Community standards: [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md). Licensed under
the [MIT License](./LICENSE).
