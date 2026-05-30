# Security Policy

Biscuit Browser is positioned as a *safer* agent browser, so security reports
are taken seriously.

## Reporting a vulnerability

**Please do not open a public issue for security vulnerabilities.**

Instead, report privately via GitHub's
[private vulnerability reporting](https://github.com/biscuitben2021-tech/Biscuit-Code/security/advisories/new)
("Report a vulnerability" under the repository's **Security** tab). If that is
unavailable, open a minimal issue asking for a private contact channel — without
details — and we'll follow up.

Please include:

- What boundary is broken and how to reproduce it.
- Impact (e.g. page content reaching the API key, escaping the gate, executing
  in the privileged main/renderer context, reading the local filesystem).
- Affected version / commit.

## Scope — boundaries that matter

The threat model centers on a hostile **web page** and on **page content trying
to drive the agent**. High-value reports include:

- Browsed-page code reaching `window.biscuit`, `ipcRenderer`, Node, or the
  decrypted API key.
- Page content modifying a **locked** Task Contract, or being treated as
  instructions by the agent.
- Bypassing the **Action Gate** to run a high-risk action without approval in
  Safe/Assisted/Auto mode.
- Arming **Bypass** without expert mode + the typed confirmation.
- Navigating to `file:`/`javascript:`/`data:` schemes (the address bar and agent
  should route these to a web search).
- Leaking the API key to any page, the renderer, or a network destination other
  than the configured model provider.

## Out of scope

- Issues that require an already-compromised local machine or a malicious model
  the user deliberately configured.
- The known, documented [limitations](./README.md#limitations-read-this)
  (cross-origin frames, closed shadow roots, `<canvas>` UIs, heuristic gating).
  Improving these is welcome as a feature PR, not a security report.

## Hardening already in place

See **Is it safe?** in the [README](./README.md#is-it-safe) for the process
isolation, single typed IPC bridge, key handling, untrusted-data treatment,
scheme restrictions, and context-aware gating that ship today.
