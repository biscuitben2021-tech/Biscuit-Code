---
name: skill-builder
description: Create a new Biscuits skill by interviewing the user about what they want, then writing a well-formed SKILL.md in the right location.
triggers:
  - make a skill
  - create a skill
  - new skill
  - build a skill
  - make your own skill
  - add a skill
  - skill builder
  - write a skill
tools:
  - Read
  - Write
  - Bash
enabled: true
---

# Skill Builder

Use this when the user wants to create a new skill ("make a skill that…",
"create a skill for…", "build your own skill"). Your job: **interview the user,
then generate a `SKILL.md`** that follows the conventions below.

## 1. Interview the user

Ask the questions below — but **only the ones they haven't already answered** in
their request. Don't interrogate; if they said "make a skill that runs our test
suite and fixes failures", you already know the purpose and roughly the steps —
just confirm the gaps. Prefer asking them together in one short list, then
proceed once you have enough.

1. **Purpose** — In one sentence, what should this skill do?
2. **When should it trigger** — What kinds of requests or keywords should switch
   it on? Get a few real example phrasings (these become the `triggers`).
3. **Workflow / rules** — The actual steps or rules the agent should follow when
   the skill is active. This is the heart of the skill — get the procedure, the
   order of operations, and any "always/never" rules.
4. **Tools** — What capabilities does it need? (`Read`, `Write`, `Edit`,
   `Grep`, `Bash`, `WebSearch`, MCP tools…) Default to the **minimum** set.
5. **Constraints / gotchas** — Anything it must NOT do, edge cases, or
   project-specific conventions to respect.
6. **Where should it live?**
   - **Project only** (per workspace, not committed): `.biscuits/skills/<name>/SKILL.md`
   - **Shared with the repo** (committed for the team): `skills/<name>/SKILL.md`
   - **Global** (just you, every project): the OS config dir —
     macOS `~/Library/Application Support/biscuits/skills/<name>/SKILL.md`,
     Linux `~/.config/biscuits/skills/<name>/SKILL.md`,
     Windows `%APPDATA%\biscuits\skills\<name>\SKILL.md`.
7. **Name** — Propose a short `kebab-case` name derived from the purpose and
   confirm it. **Enabled now?** (default: yes.)

## 2. Write the SKILL.md

A skill is one folder containing a `SKILL.md`: optional YAML frontmatter, then a
Markdown body. Use this exact frontmatter schema (all fields except none are
required, but fill them all in for a good skill):

```markdown
---
name: <kebab-case-name>            # else inferred from the folder name
description: <one concise sentence — used to match the skill to a request>
triggers:                          # short phrases/keywords that should select it
  - <phrase from the user's examples>
  - <keyword>
tools:                             # the minimum capabilities it needs
  - Read
  - Bash
enabled: true                      # false to ship it disabled
---

# <Title Case Name>

Use this when <the situation / kinds of requests that should trigger it>.

## Approach

1. <First concrete, imperative step.>
2. <Next step — real commands/paths where relevant.>
3. <…verify / report.>

## Notes

- <Constraint, gotcha, or "always/never" rule.>
```

Then create it:

1. Choose the path from the location answer and create the folder + file with
   the `Write` tool (e.g. `skills/<name>/SKILL.md`). If you can't write to a
   global OS path, write it and tell the user, or fall back to the project path
   and say so.
2. If the folder might not exist, that's fine — `Write` creates parent dirs.

## 3. Validate & confirm

- Re-read the file you wrote and check the frontmatter parses (valid YAML, lists
  use `- ` items) and the body has a clear title + numbered steps.
- Tell the user the path, then how to use it:
  `/skills refresh` (reload from disk), `/skills` (list + status/source),
  `/skills show <name>` (inspect), `/skills selected <message>` (debug whether a
  message would select it), `/skills enable|disable <name>`.

## What makes a good skill

- **Imperative and concrete.** "Run `cargo test`, read the first error, fix the
  root cause" beats "help with testing". Include real commands, file paths, and
  the order to do things in.
- **Triggers that match how people actually ask.** Use the user's own example
  phrasings plus the obvious keywords — selection scores triggers and the name
  most strongly, then description keywords.
- **Focused.** One skill = one job. If the user describes two unrelated jobs,
  offer to make two skills.
- **Minimal tools.** Only list the tools the workflow truly needs.
- **Honest scope.** Note what it does *not* handle in `## Notes` rather than
  implying it does everything.
- **Guidance, not gospel.** Skills are advisory — the user's latest instructions
  always override them; write accordingly.

## Notes

- `name` must be unique; if it collides with an existing skill, the
  higher-precedence location wins (project > repo > global). Pick a distinct
  name or confirm the override with the user.
- Enable/disable state lives in `.biscuits/skills.json`; never edit a skill's
  enabled state by hand-editing other skills — only set this skill's own
  `enabled:` field.
- If the user wants a *format-conversion* skill specifically, point them at the
  existing `format-converter` skill instead of duplicating it.
