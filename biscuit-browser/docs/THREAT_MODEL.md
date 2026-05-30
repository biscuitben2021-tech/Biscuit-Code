# Biscuit Browser — Threat Model

Biscuit Browser is positioned as a *safer* agent browser. "Safer" is a claim
that only means something if we are explicit about what we trust, what we don't,
what we guard, and what we have **not** solved. This document is that contract.

> **We do not claim to "prevent prompt injection."** No agent browser can today.
> Biscuit aims to **reduce prompt-injection risk** and make agent actions
> inspectable and revocable, through the Agent View, the locked Task Contract,
> the Action Gate, and permission modes.

## Trust boundaries

### Trusted

- The **user's original prompt**.
- The **application code** (main process, preload, renderer UI).
- The **locked Task Contract** — generated from the user's prompt only, immutable
  for the duration of the task.
- The **API key**, which lives only in the main process (encrypted at rest, or
  session-only when the OS keychain is unavailable).

### Untrusted

- **Web pages** and everything derived from them: page text, the Agent View,
  DOM text, link/button labels, headings, search results, screenshots.
- **Model output** is treated as a *proposal*, not a command — every action is
  re-classified and gated regardless of what the model "intended".

The system prompt tells the model that page content is untrusted data and must
never be followed as instructions. The locked contract — not the page — defines
what the agent is allowed to do.

## Guarded actions

Every proposed action is classified and checked by the Action Gate against the
permission mode and the locked contract. The contract action vocabulary:

```
open · read · search · scroll · click · type · submit
login · payment · upload · download · send · delete · settings
```

By default (the conservative contract): research actions (open/read/search/
scroll/click) are allowed; `type`/`submit`/`upload`/`download` require
confirmation; `login`/`payment`/`send`/`delete`/`settings` are blocked without
an explicit user override. Sensitive fields (password/card/cvv/ssn/otp…) always
require confirmation, and **their values are never sent to the model or shown in
the UI**. Page context (checkout/login/account, detected from URL/title/text)
escalates otherwise-ambiguous clicks like "Continue" / "Confirm".

## Process & isolation

- `contextIsolation`, `sandbox`, `nodeIntegration: false` for the app UI **and**
  every browsed page.
- Browsed pages run in a separate session with **no preload** — they cannot
  reach app APIs or the key.
- The only renderer↔main bridge is the typed `window.biscuit` surface. No raw
  Node, no `ipcRenderer`, no arbitrary channels.
- The address bar and the agent can only navigate `http(s)` / `about:blank`;
  `file:` / `data:` / `javascript:` / etc. are routed to a web search.

## Not solved yet (known gaps)

Be skeptical of any agent browser — including this one — about:

- **Prompt injection** is *reduced, not eliminated*. A page that tricks the model
  into proposing an in-contract action can still succeed within the contract's
  bounds.
- **CAPTCHAs** and bot-detection are not handled.
- **Banking / high-stakes financial sites**: do not run the agent on these. The
  gate escalates, but treat money flows as out of scope.
- **Cross-origin iframes**, **closed shadow roots**, and **`<canvas>`-painted
  UIs** are invisible to the Agent View extractor (reported honestly in the
  snapshot's `context.notes`).
- **Malicious downloads** are not scanned; `download` is gated but the file is
  not inspected.
- **Verification is heuristic** — it catches navigations, content changes, and
  new error/validation banners, not every silent failure.
- **The gate is heuristic and English-centric** — novel phrasings or non-English
  labels may be under-classified. The contract + your chosen mode are the
  backstop.

## Reporting

Security issues: see [../SECURITY.md](../SECURITY.md). Please report privately.
