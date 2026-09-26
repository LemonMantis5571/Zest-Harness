---
title: Skills and slash commands
summary: Personal SKILL.md files become /commands and model-readable instructions; enabled MCP servers and built-ins share the same / list in the composer.
paths:
  - crates/core/src/skills.rs
  - crates/core/src/tools/read_skill.rs
  - crates/core/src/commands.rs
  - crates/desktop/ui/src/lib/slashCommands.ts
  - crates/desktop/ui/src/components/CommandOutputCard.tsx
  - crates/desktop/ui/src/components/CustomizePanel.tsx
tests:
  - crates/desktop/ui/src/lib/slashCommands.test.ts
  - crates/desktop/ui/e2e/slash-commands.spec.ts
verify:
  - cargo test -p zest-core --lib -- skills::tests
  - cargo test -p zest-core --lib -- tools::read_skill
  - cargo test -p zest-core --lib -- commands::tests
  - npm run ui:test
  - npm run ui:e2e -- slash-commands
---

# Skills and slash commands

## Behavior
- Skills are personal: only `~/.agents/skills/<dir>/SKILL.md` and
  `~/.zest/skills/<dir>/SKILL.md` are loaded (the second wins on a name clash).
  Project folders are never loaded as skills (docs/SKILLS.md).
- A skill needs frontmatter with `name` and `description`; files over 64 KiB or
  missing either field are skipped with a warning. At most 32 skills are kept.
- The system prompt lists every skill (`# Available skills`) and inlines bodies of
  4 KiB or less up to a 16 KiB total (`# Skill details`); the model loads the rest
  with `read_skill(name)`, which errors with the known names on a miss.
- In the composer, `/` at the start of a whitespace-delimited token (before the
  caret) opens the Commands list, filtered by name prefix. Paths such as
  `/etc/hosts` do not open it. Several commands can be used in one draft.
- The list contains built-ins `btw` and `model`, skills, and enabled MCP servers
  whose id is a legal token. Reserved built-ins beat skills and MCP; a skill beats
  an MCP server of the same name.
- On send, every recognised `/token` anywhere in the draft is removed and its
  skill body (or an MCP "use this server" instruction) is placed above the
  remaining text, separated by `---`. Unknown `/tokens` stay as typed. `//text`
  sends a literal `/text`.
- The transcript and thread title keep what the user typed; the expansion goes
  only to the model. The message records the command names (`plan + review`) and
  a document-shaped reply renders in `CommandOutputCard`.
- Customize > Skills lists the skills found on disk; Refresh re-reads them.

## How it works
1. `SkillSet::discover` (skills.rs) runs in `RuntimeBuilder::build`
   (runtime.rs); `compose_system_with_docs` (prompt.rs) adds the catalogue and
   inlined bodies; `register_skill_tools` adds `ReadSkill` over the shared
   `Arc<RwLock<SkillSet>>`.
2. Composer.tsx loads `backend.listCommands()` on mount and again whenever the
   palette opens -> Tauri `list_commands` (crates/desktop/src/lib.rs), which calls
   `SkillSet::discover()`, `mcp_slashes`, and `list_slash_commands`.
3. `slashTokenAt`, `filterSlashCommands`, `splitSlashMatch` (slashCommands.ts)
   drive the list; `isModelSlash` / `btwQuestion` route the built-ins in the UI.
4. On send, turn.rs calls `zest_core::expand_command(text, &session.skills,
   &mcp)` (`commands::expand`); in Plan mode with no typed command it uses
   `expand_command_as(text, skills, PLAN_SKILL)`.
5. Customize > Skills (`SkillsPanel` in CustomizePanel.tsx) calls Tauri
   `list_skills`, which reads skills from disk so it works mid-turn.

## Verify
Unit tests above; the e2e spec inserts `/browse-x` and `/plan` at several caret
positions in `?fixture=1` (fixture skills come from fixtureBackend.ts
`listCommands`).

## Pitfalls
- `list_commands` and `list_skills` re-discover from disk each call, but expansion
  uses the session's `SkillSet`, discovered when the runtime was built. A skill
  added mid-session shows in the list before the session can expand it.
- Command names accept only `[A-Za-z0-9_-]`; everything else must survive
  untouched (paths, URLs, regexes).
- Skill bodies are instructions to the model; only install trusted skills.
