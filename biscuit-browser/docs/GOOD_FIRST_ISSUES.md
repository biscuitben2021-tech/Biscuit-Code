# Good first issues

Starter contributions for Biscuit Browser. File these as GitHub issues labeled
`good first issue`, or just open a PR. See [../CONTRIBUTING.md](../CONTRIBUTING.md).

- **Capture the README screenshots + demo GIF** (`docs/` has a capture guide).
- **Add an Agent View export button** — download the current snapshot as JSON.
- **Add a dark/light theme toggle** (the UI is currently dark-only).
- **More Action Gate unit tests** — new risky phrasings / page contexts.
- **More URL normalization tests** — punycode, ports, IDN, userinfo.
- **Extractor fixture pages** — a custom dropdown, a nested shadow root, an ARIA
  combobox; assert what the agent should see (`test/extractor.test.ts`).
- **Covered-element edge cases** — sticky headers / cookie walls over a target.
- **First-run onboarding polish** — remember "don't show again", add a skip.
- **Better empty/error states** in the side panels.
- **Demo mode scenarios** — add a second synthetic page (e.g. a fake login form)
  to show the gate blocking a sensitive action.

When you pick one up, comment on the issue so others don't duplicate work.
