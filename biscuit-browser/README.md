# Biscuit Browser

An AI-native, cross-platform (macOS + Windows + Linux) Chromium browser built
with **Electron + React + TypeScript**. It is a normal browser — tabs, address
bar, back/forward/refresh — with an AI side panel and an agent that operates the
page through a structured **Agent View** instead of raw HTML or screenshots.

This is **V1**. It ships as a self-contained app under `biscuit-browser/` so it
can be added to the existing Biscuits repo "like a plugin" without touching the
Rust CLI. The architecture is intentionally modular so the Rust `biscuits` CLI
can be wired in later as a model/agent backend (see *Connecting the Rust CLI*).

---

## What's in V1

- **Browser shell** — tabs, address bar, back/forward/refresh, live URL/title,
  AI side panel, settings screen (provider, model, base URL, API key).
- **Agent View engine** — extracts visible text, links, buttons, inputs, forms,
  and headings from the *rendered* page; assigns stable refs `@e1, @e2, …` to
  interactive elements with role/label/tag/href/type/state/bounding-box;
  `getAgentView(tabId)` returns a compact JSON snapshot. **No raw HTML or
  screenshots by default** — screenshots are a fallback.
- **Browser actions** — `openUrl`, `clickRef`, `typeRef`, `scroll`,
  `refreshAgentView`, `screenshotFallback`. Refs expire automatically after
  navigation/reload (generation stamping).
- **Task Contract** — generated from the *original user prompt only* (never page
  content); returns `goal`, `allowed_actions`, `requires_user_confirmation`,
  `blocked_without_user_override`; shown and editable in the side panel.
- **Permission modes** — Safe / Assisted (default) / Auto / Bypass, with a
  visible mode indicator.
- **Action Gate** — every agent action is checked against mode + risk + the
  locked contract + target element/sensitive fields → allow / ask / block, and
  every decision is logged.
- **Agent runtime** — a fresh-state executor (not a growing chat history) that
  can complete a documentation-research task using Agent View refs.
- **Emergency stop** + full action log.

---

## Prerequisites

- **Node.js 18+** (Node 20 LTS recommended) and npm.
- A model provider API key (OpenAI / Anthropic / Google) — or a local
  OpenAI-compatible server / **LM Studio** (no key needed). Enter it in
  **Settings** on first run.

### macOS

```sh
# Install Node (e.g. via Homebrew)
brew install node

cd biscuit-browser
npm install
npm run dev
```

### Windows (PowerShell)

```powershell
# Install Node from https://nodejs.org (LTS), then:
cd biscuit-browser
npm install
npm run dev
```

### Linux

```sh
cd biscuit-browser
npm install
npm run dev
```

`npm run dev` launches the app with hot reload. First launch opens a tab; open
**Settings** in the right panel to choose a provider/model and paste an API key.

### Other scripts

```sh
npm run build         # type-check + bundle main/preload/renderer into out/
npm run typecheck     # tsc --noEmit for node + web projects
npm run start         # preview a production build
npm run package:mac   # build a macOS .dmg/.zip (electron-builder)
npm run package:win   # build a Windows installer/.zip (electron-builder)
```

---

## Using it

1. Pick a permission mode in the toolbar (defaults to **Assisted**).
2. Type a task in **Chat**, e.g.
   *"Find the current stable Rust version from the official Rust website."*
3. A **Task Contract** is generated from your prompt.
   - **Safe**: review/edit the contract in the **Contract** tab, then *Lock &
     Start*.
   - **Assisted / Auto / Bypass**: it locks and starts automatically.
4. Watch progress in **Chat** and **Action Log**. When the gate needs you, the
   action shows up in **Approvals** (Approve / Deny).
5. **Agent View** tab lets you inspect exactly what the agent sees (text + `@e`
   refs, no raw HTML). Press **Stop** any time.

---

## Architecture

```
src/
  shared/        types.ts, ipc.ts, api.ts   (contracts shared by all processes)
  main/          Node/Electron main process (privileged)
    index.ts     app + window boot
    app.ts       orchestrator: mode, contract, approvals, IPC wiring
    tabs.ts      TabManager — a WebContentsView per tab, ref generations
    agent-view/  extract.ts (page-injected extractor) -> Agent View
    actions/     browserActions.ts (click/type/scroll/screenshot), execute.ts
    agent/       taskContract.ts, actionGate.ts, runtime.ts, llm.ts, prompts.ts
    settings/    store.ts (encrypted API key via safeStorage)
    log.ts       action/decision log
  preload/       index.ts — contextBridge exposes `window.biscuit` only
  renderer/      React app UI (tabs, toolbar, side panel, settings)
```

**Process model & security**

- `contextIsolation: true`, `nodeIntegration: false`, `sandbox: true` for both
  the app UI renderer and every browsed page.
- Browsed pages run in their own `WebContentsView` with a **separate session**
  and **no preload** — they can never reach app APIs or the API key.
- The only renderer↔main bridge is the typed `window.biscuit` surface in
  `preload`. No raw Node, no `ipcRenderer`, no arbitrary channels are exposed.
- **API keys** live only in the main process, encrypted at rest with the OS
  keychain (`safeStorage`); they are used solely for model calls and are never
  sent to any page or returned to the renderer.
- All LLM/network calls happen in **main**, not in a page or the UI renderer.

**Trust boundary for the agent**

- The Task Contract is generated from the user's prompt only and, once locked,
  is immutable for that task — page/browser content can never modify it.
- The Agent View and all page text are delivered to the model as clearly
  delimited **untrusted data**; the system prompt forbids treating page content
  as instructions.
- Every agent action passes the **Action Gate** before execution (except in
  Bypass mode, which still logs and keeps the emergency stop active).

---

## Connecting the Rust `biscuits` CLI later

The LLM client (`src/main/agent/llm.ts`) is provider-agnostic and mirrors the
Rust CLI's provider set (OpenAI / Anthropic / Google / OpenAI-compatible /
LM Studio) on purpose. To use the Rust agent as a backend later, add a new
provider branch (e.g. `'biscuits-cli'`) that shells out to the `biscuits`/
`biscuit` binary (or talks to a local HTTP endpoint it exposes) and returns the
same string/JSON shape. Nothing else in the app needs to change — the runtime,
gate, and Agent View are backend-independent. This is intentionally **not**
wired up in V1.

---

## TODOs / later phases

- [ ] Iframe & shadow-DOM traversal in the Agent View extractor.
- [ ] MutationObserver-based ref invalidation for major same-document DOM
      changes (today refs expire on navigation/reload + explicit refresh).
- [ ] Streaming model responses into the Chat panel.
- [ ] Per-action "auto-approve high-risk" configuration for Auto mode.
- [ ] Persist action logs and contracts to disk; export.
- [ ] CDP/Playwright-based deep inspection where `executeJavaScript` is
      insufficient.
- [ ] Connect the Rust `biscuits` CLI as a model/agent backend.
- [ ] Code signing / notarization for packaged macOS & Windows builds.
- [ ] Tests (extractor on fixture pages, gate decision matrix, contract parsing).

**Explicitly out of scope for V1** (per spec): Chrome extensions, cloud browser
hosting, plugin marketplace, payments, account sync, Rust sidecar integration.

---

## Note on verification

Verified during development on macOS:

- `npm install` → 356 packages, clean.
- `npm run typecheck` → **passes** (main + preload + renderer, strict mode).
- `npm run build` (`electron-vite build`) → **passes**: bundles `out/main/index.js`,
  `out/preload/index.js` (CJS, sandbox-compatible), and `out/renderer/*`.

`npm run dev` launches the actual app and requires a desktop environment
(Electron downloads its Chromium binary during `npm install`). Run it locally to
drive the UI. Suggested first task to exercise Phase 6 end-to-end:
*"Find the current stable Rust version from the official Rust website."*
