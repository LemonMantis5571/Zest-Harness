# Skills

Skills belong to the person using Zest, not to the project.

Zest loads folders that contain `SKILL.md` from:

- `%USERPROFILE%\.agents\skills\<name>\SKILL.md`
- `%USERPROFILE%\.zest\skills\<name>\SKILL.md`

On macOS and Linux, use `~/.agents/skills/` and `~/.zest/skills/`.

Project folders such as `.zest/skills/`, `.agents/skills/`, and `skills/` are
not loaded by Zest as user skills. A repository may keep its own `skills/`
folder for contributor or agent instructions, but those files are not part of
the app's skill catalog. Personal skills stay outside the repository so every
user can install their own set.

Each skill needs this small frontmatter block:

```md
---
name: my-skill
description: Short sentence about when to use it
---

Instructions for the agent go here.
```

The folder name does not have to match `name`, but names must be unique. A
skill can be used as `/my-skill` in chat. Enabled MCP servers share that `/`
list; a skill of the same name wins. Small skill bodies may be included in
the system prompt; larger bodies are read only when needed.

## Install

Copy a skill folder into one of the user folders above, then reopen Zest or
press **Refresh** in **Customize > Skills**, which lists the skills currently
found on this computer.

Zest treats skill files as instructions. Only install skills you trust.
