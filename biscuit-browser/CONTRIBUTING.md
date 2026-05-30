# Contributing to Biscuit Browser

Thanks for considering a contribution! Biscuit Browser is an experimental,
safety-focused AI browser, and the most valuable contributions make the agent
**more capable without making it less safe** — or make the safety layer easier
to trust.

## Getting set up

```sh
cd biscuit-browser
npm install
npm run dev        # launch the app
```

Requires Node.js 18+ (20 LTS recommended).

## Before you open a PR

Everything below runs in CI on Linux, macOS, and Windows — run it locally first:

```sh
npm run typecheck  # strict tsc (main + preload + renderer)
npm test           # vitest unit tests for the safety layer
npm run build      # electron-vite bundle must succeed
```

PRs that touch the safety layer (the Action Gate, Task Contract, Agent View
extractor, permission modes, verification, or sensitive-field detection)
**must** include or update tests in `test/`.

## Good first issues

- **Add a fixture page to the extractor tests.** `test/extractor.test.ts` runs
  the page-world extractor against jsdom. Add a tricky DOM (custom dropdown,
  nested shadow root, ARIA widget) and assert what the agent should see.
- **Extend the gate decision matrix.** New risky phrasings, new page contexts.
- **A screenshot / GIF walkthrough** for the README (`docs/`).
- **Cross-origin frame handling** via a CDP/Playwright controller (see the
  roadmap in the README) — a larger, high-impact piece.

## Code style & expectations

- TypeScript, strict mode. No `any` in shipped code unless genuinely necessary
  (the page-world extractor scripts are `@ts-nocheck` on purpose — they run in
  the page, not Node).
- Keep `src/shared/` free of runtime imports so it stays usable in every process
  and easy to unit-test.
- **Security boundaries are not negotiable.** Don't widen the `preload`
  (`window.biscuit`) surface without a clear reason, don't pass the API key to
  the renderer or a page, and don't let page content influence a locked
  contract. If a change relaxes a boundary, say so explicitly in the PR.
- Match the surrounding code's comment density and naming.

## Reporting bugs / requesting features

Open an issue at
<https://github.com/biscuitben2021-tech/Biscuit-Code/issues>. For anything that
looks like a security vulnerability, follow [SECURITY.md](./SECURITY.md) instead
of filing a public issue.
