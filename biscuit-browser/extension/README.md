# Biscuit Agent — browser extension (Chrome & Safari)

Brings the Biscuit agent **into Chrome and Safari**. The agent reads the current
page as a structured **Agent View** (interactive elements with `@e` refs — not
pixels), then clicks / types / scrolls to do your task, gated by a permission
mode and an Action Gate (high-risk actions need your approval). By default it
runs on a **free local model** (LM Studio), so there's no API cost.

It is plain JavaScript (Manifest V3) with **no build step** — load the folder
directly.

## What's inside
- `manifest.json` — MV3 manifest (least-privilege: `activeTab`, `scripting`, `storage`, `sidePanel`; host access limited to `localhost` for the local model).
- `content.js` — runs in the page: extracts the Agent View and executes actions (adapted from the Electron app's `agent-view/extract.ts` + `actions/browserActions.ts`).
- `background.js` — the agent loop (view → LLM → gate → act → repeat), with loop guards.
- `gate.js` — the Action Gate (risk classification + mode → allow/ask).
- `llm.js` — multi-provider chat client (default OpenAI-compatible / LM Studio).
- `sidepanel.html` / `sidepanel.js` — the chat side panel + inline approvals.
- `options.html` / `options.js` — configure provider / model / mode.

## Load in Chrome (or Edge/Brave)
1. Open `chrome://extensions`.
2. Turn on **Developer mode** (top right).
3. **Load unpacked** → select this `extension/` folder.
4. Click the Biscuit Agent toolbar icon to open the **side panel**.
5. Open **Settings** (link in the panel) — the defaults target LM Studio at
   `http://localhost:1234/v1`. Start LM Studio (or any OpenAI-compatible server)
   and load a model, or switch the provider and add an API key.
6. Go to any page, type a task ("find the cheapest option and add it to cart"),
   and press **Run**. Keep the mode on **Safe** or **Assisted** for real sites.

> Restricted pages (`chrome://…`, the Chrome Web Store, some PDF viewers) block
> content scripts — the agent can't act there.

## Package for Safari (macOS)
Safari supports MV3 web extensions via a converter:

```sh
xcrun safari-web-extension-converter biscuit-browser/extension --project-location ~/Desktop/BiscuitAgentSafari --macos-only
```

Open the generated Xcode project, build & run, then enable the extension in
**Safari → Settings → Extensions** (you may need "Allow unsigned extensions" in
the Develop menu during development). The code uses a `globalThis.browser ?? chrome`
shim so the same source runs in both browsers.

## Using a hosted provider
To call a cloud LLM (OpenAI/Anthropic/Google) instead of a local model, set the
provider + key in Settings **and** add the provider's host to `host_permissions`
in `manifest.json` (e.g. `"https://api.openai.com/*"`), then reload the extension.

## Security notes
- The agent operates your **real, logged-in** tabs. The Action Gate escalates
  risk on payment/login/destructive contexts and requires approval for high-risk
  actions; **never** lower the mode to Auto/Bypass for untrusted tasks.
- Field **values** of sensitive inputs (passwords, card numbers) are never read
  into the Agent View. All page text is treated as untrusted data, never as
  instructions.
- An API key set here lives in extension storage, which is **less protected than
  an OS keychain** — prefer a local model (no key).

## Status
This ships as source; it has not been load-tested in a live browser from CI.
Load it unpacked (above) to verify behavior on your machine.
