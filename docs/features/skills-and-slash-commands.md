---
title: Skills and slash commands
summary: Personal SKILL.md files become /commands and model-readable instructions; built-ins, skills and enabled MCP servers are listed in Customize > Skills & MCP.
paths:
  - crates/core/src/skills.rs
  - crates/core/src/tools/read_skill.rs
  - crates/core/src/commands.rs
  - crates/desktop/src/native.rs
  - crates/desktop/src/native_view.rs
  - crates/desktop/src/workspace.rs
tests:
  - crates/desktop/src/workspace_test.rs
verify:
  - cargo test -p zest-core --lib -- skills::tests
  - cargo test -p zest-core --lib -- tools::read_skill
  - cargo test -p zest-core --lib -- commands::tests
  - npm run gpui:test
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
- The composer does not open a command list when you type `/`; its placeholder
  only mentions commands. Type the command yourself, or pick it from
  Customize > Skills & MCP, which lists every command as `/name · description`
  ("No skills or MCP commands configured." when there are none). Clicking a row
  returns to the chat and puts `/<name> ` into the composer.
- The list contains built-ins `btw` and `model`, skills, and enabled MCP servers
  whose id is a legal token. Reserved built-ins beat skills and MCP; a skill beats
  an MCP server of the same name.
- Sending exactly `/model` opens Customize > Models (see
  [model-and-effort-selection](model-and-effort-selection.md)); `/btw <question>`
  opens the side conversation (see [btw-side-questions](btw-side-questions.md)).
  Neither is sent as a turn.
- On send, every recognised `/token` anywhere in the draft is removed and its
  skill body (or an MCP "use this server" instruction) is placed above the
  remaining text, separated by `---`. Unknown `/tokens` stay as typed. `//text`
  sends a literal `/text`. Several commands can be used in one draft; paths such
  as `/etc/hosts` are left alone.
- In Plan mode a message with no recognised command is expanded with the `plan`
  skill.
- The transcript keeps what the user typed; the expansion goes only to the
  model.
- The offline demo loads no skills and no MCP commands.

## How it works
1. `SkillSet::discover` (skills.rs) runs in `RuntimeBuilder::build`
   (runtime.rs), which the desktop calls for every turn; `compose_system_with_docs`
   (prompt.rs) adds the catalogue and inlined bodies; `register_skill_tools` adds
   `ReadSkill` over the shared `Arc<RwLock<SkillSet>>`.
2. `Project::open_with_registry` (`native.rs`) discovers a second `SkillSet`
   and the MCP slashes (`mcp_slashes(&config.mcp, &McpCatalog::load())`) once
   per project window and keeps them in `Project.skills` / `Project.mcp`.
3. Customize > Skills & MCP (`Panel::Commands` in `workspace.rs`) renders
   `zest_core::list_slash_commands(&project.skills, &project.mcp)`.
4. `Root::submit_native` (`native_view.rs`) catches `/model` and
   `btw_question(text)` before queueing.
5. When a queued input runs, `Chat::run` (`native.rs`) calls
   `zest_core::expand_command(text, &project.skills, &project.mcp)`
   (`commands::expand`); in Plan mode with no typed command it uses
   `expand_command_as(text, skills, "plan")`.

## Verify
Unit tests above. `npm run gpui:test` renders the Skills & MCP panel during the
workspace selftest. By hand: add a skill under `~/.zest/skills/`, open a project,
pick it in Customize > Skills & MCP, and send.

## Pitfalls
- The Skills & MCP list and `/command` expansion use the window's `SkillSet`,
  discovered when the project window opened, while the system prompt and
  `read_skill` rediscover skills every turn. A skill added mid-session is
  visible to the model but is not a `/command` until the window is reopened.
  There is no Refresh button.
- Command names accept only `[A-Za-z0-9_-]`; everything else must survive
  untouched (paths, URLs, regexes).
- Skill bodies are instructions to the model; only install trusted skills.
