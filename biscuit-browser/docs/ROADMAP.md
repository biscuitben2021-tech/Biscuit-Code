# Biscuit Browser — Roadmap

Biscuit Browser is **experimental**. This roadmap is intentionally honest about
what exists, what's next, and what's deliberately out of scope.

## V1 — now (experimental)

Shipped and unit-tested:

- Chromium browser shell: tabs, address bar, back/forward/reload, AI side panel.
- **Agent View** engine: visible text + interactive elements as stable `@e` refs
  (role/label/state/box), traversing open shadow roots and same-origin iframes,
  with an honest coverage report. No raw HTML / screenshots by default.
- **Task Contract** generated from the user prompt only; locked + immutable.
- **Action Gate**: page-context-aware risk classification → allow / ask / block.
- **Permission modes**: Safe / Assisted / Auto / Bypass (Danger), with
  expert-mode gating + typed confirmation for Bypass.
- **Verification layer**: before/after page fingerprint to confirm an action
  took effect (and didn't trip an error / validation message).
- Encrypted (or session-only) API key handling; full Action Log; emergency Stop.
- Demo mode (no API key required) to explore the product offline.
- Vitest test suite + CI (Linux/macOS/Windows).

## V2 — next

- **CDP / Playwright controller** for robust control and cross-origin frames
  where `executeJavaScript` is insufficient.
- **Real input events** for clicks/typing (pointer/keyboard) instead of
  synthetic `el.click()` / value-set.
- **MutationObserver-based ref invalidation** for same-document DOM changes.
- **Streaming** model responses into Chat.
- **Persisted + exportable** action logs and contracts.
- **Per-action "auto-approve high-risk" config** for Auto mode.
- Stronger, structured runtime state + a dedicated verifier step.

## Later

- Connect the Rust `biscuits` CLI as a model/agent backend.
- Code signing / notarization for packaged macOS & Windows builds; auto-update;
  release checksums.
- Localized / multilingual risk classification.
- Optional vision fallback for `<canvas>`-heavy apps.

## Out of scope (for now)

Chrome extensions, cloud browser hosting, a plugin marketplace, account sync,
solving CAPTCHAs, and running on banking / high-stakes financial sites.

See also the [Threat Model](./THREAT_MODEL.md) for known gaps.
