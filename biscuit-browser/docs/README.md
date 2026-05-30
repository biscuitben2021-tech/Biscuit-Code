# docs/ — media & reference assets

This folder holds the screenshots, demo recording, and reference docs the README
links to.

## Demo recording (`docs/demo.gif`)

The main README embeds `![Biscuit Browser demo](docs/demo.gif)` near the top.
**This file is not committed yet** — it has to be recorded from the running app
(a code agent can't capture it). To make it:

1. `npm run dev` and complete first-run onboarding.
2. Pick a provider (or use **Demo mode** — no key needed: side panel → Chat →
   "Run keyless demo").
3. Record a ~15–25s screen capture and export as `docs/demo.gif`
   (macOS: [Kap](https://getkap.co/) or `⌘⇧5`; then convert to GIF, or use a
   webm/mp4 and update the README link).

Show, in order:

```
normal browser view  →  AI side panel  →  Agent View refs (@e1, @e2…)
→  Task Contract  →  permission mode selector  →  an approval prompt  →  Action Log
```

## Screenshots (`docs/img/*.png`)

Capture these and reference them in the README "Screenshots" section:

```
docs/img/browser.png         normal browser + side panel
docs/img/agent-view.png      Agent View panel (refs + coverage notes)
docs/img/contract.png        Task Contract panel (allowed/confirm/blocked)
docs/img/approvals.png       an approval prompt (gate decision + reason)
docs/img/bypass.png          the Bypass (Danger) banner + confirm modal
docs/img/action-log.png      Action Log with gate decisions
```

Until they exist, the README notes them as "pending" rather than showing broken
images.

## Reference docs

- [THREAT_MODEL.md](./THREAT_MODEL.md) — trust boundaries and known gaps.
- [ROADMAP.md](./ROADMAP.md) — V1 / V2 / later.
- [GOOD_FIRST_ISSUES.md](./GOOD_FIRST_ISSUES.md) — starter contributions.
