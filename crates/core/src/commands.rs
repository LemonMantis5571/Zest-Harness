//! Slash commands, backed by skills and enabled MCP servers.
//!
//! A command is not a new concept — it is a named thing invoked from the
//! composer. `/plan` means "run a personal skill against what I typed next",
//! so a new skill command is a markdown file rather than a code change.
//! `/haiku` means "use that MCP server for this request", so a new server
//! command is an enabled `[mcp.<id>]` entry.
//!
//! Parsing is deliberately narrow. Only a token at the very start of the
//! message counts, and only `[a-z0-9-_]`, because everything else people type
//! at the start of a line — a path, a regex, a URL — must survive untouched.

use std::collections::BTreeMap;

use crate::config::McpServerConfig;
use crate::mcp::McpCatalog;
use crate::skills::SkillSet;

/// A leading `/token` split off the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand<'a> {
    pub name: &'a str,
    /// Whatever followed the token, trimmed. Often empty.
    pub rest: &'a str,
}

/// Split a leading `/token` off `input`.
///
/// `None` for anything that is not a command, which includes the escape form:
/// a message starting `//` is a literal slash, because sooner or later someone
/// starts a message with a path.
pub fn parse_command(input: &str) -> Option<ParsedCommand<'_>> {
    let trimmed = input.trim_start();
    let after_slash = trimmed.strip_prefix('/')?;

    // `//foo` is an escape, not a command named `/foo`.
    if after_slash.starts_with('/') {
        return None;
    }

    let end = after_slash
        .find(|c: char| !is_name_char(c))
        .unwrap_or(after_slash.len());
    let name = &after_slash[..end];
    if name.is_empty() {
        return None;
    }

    Some(ParsedCommand {
        name,
        rest: after_slash[end..].trim(),
    })
}

/// Strip the `//` escape so the model sees the single slash the user meant.
pub fn unescape(input: &str) -> String {
    let trimmed = input.trim_start();
    match trimmed.strip_prefix("//") {
        Some(rest) => format!("/{rest}"),
        None => input.to_string(),
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// True when `id` can be typed as `/id`.
pub fn is_command_name(id: &str) -> bool {
    !id.is_empty() && id.chars().all(is_name_char)
}

/// An enabled MCP server that can be invoked as `/id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSlash {
    pub id: String,
    pub description: String,
    pub tools: Vec<String>,
}

/// What the composer lists for one `/` match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashKind {
    Skill,
    Mcp,
    Builtin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub kind: SlashKind,
}

/// Enabled MCP servers whose ids are legal slash tokens.
pub fn mcp_slashes(
    servers: &BTreeMap<String, McpServerConfig>,
    catalog: &McpCatalog,
) -> Vec<McpSlash> {
    servers
        .iter()
        .filter(|(id, config)| config.enabled && is_command_name(id))
        .map(|(id, _)| {
            let tools = catalog
                .servers
                .get(id)
                .map(|entry| {
                    entry
                        .tools
                        .iter()
                        .map(|tool| tool.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            McpSlash {
                id: id.clone(),
                description: format!("Use the {id} MCP server"),
                tools,
            }
        })
        .collect()
}

/// Built-in names that are not skills or MCP servers. A skill named `model`
/// would steal `/model` from the picker, so reserved names win here.
const BUILTIN_COMMANDS: &[(&str, &str)] = &[("model", "Switch model or provider")];

fn is_reserved_command(name: &str) -> bool {
    BUILTIN_COMMANDS
        .iter()
        .any(|(reserved, _)| reserved.eq_ignore_ascii_case(name))
}

/// Skills first on a name clash with MCP. Reserved builtins beat both.
pub fn list_slash_commands(skills: &SkillSet, mcp: &[McpSlash]) -> Vec<SlashCommand> {
    let mut commands: Vec<SlashCommand> = BUILTIN_COMMANDS
        .iter()
        .map(|(name, description)| SlashCommand {
            name: (*name).to_string(),
            description: (*description).to_string(),
            kind: SlashKind::Builtin,
        })
        .collect();
    for (name, description) in skills.command_names() {
        if is_reserved_command(&name) {
            continue;
        }
        commands.push(SlashCommand {
            name,
            description,
            kind: SlashKind::Skill,
        });
    }
    for server in mcp {
        if is_reserved_command(&server.id) {
            continue;
        }
        let taken = commands
            .iter()
            .any(|command| command.name.eq_ignore_ascii_case(&server.id));
        if taken {
            continue;
        }
        commands.push(SlashCommand {
            name: server.id.clone(),
            description: server.description.clone(),
            kind: SlashKind::Mcp,
        });
    }
    commands.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    commands
}

fn lookup_mcp<'a>(mcp: &'a [McpSlash], typed: &str) -> Option<&'a McpSlash> {
    mcp.iter()
        .find(|server| server.id.eq_ignore_ascii_case(typed))
}

/// What a command expands to, and what the transcript should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    /// Sent to the model.
    pub prompt: String,
    /// Shown in the transcript and used for the thread title — the text as
    /// typed. Storing the expansion instead would make every `/plan` chat look
    /// identical in the sidebar and bloat persisted history.
    pub display: String,
    /// `Some` when a command was recognised and applied.
    pub command: Option<String>,
}

/// Expand a leading command against the available skills and MCP servers.
///
/// An unrecognised `/token` is passed through unchanged rather than rejected: a
/// typo should not swallow the message, and the model can say it did not
/// understand far more usefully than an error dialog can. A skill with the
/// same name as an MCP server wins, because `/plan` is already a skill.
pub fn expand(input: &str, skills: &SkillSet, mcp: &[McpSlash]) -> Expansion {
    let Some(parsed) = parse_command(input) else {
        return Expansion {
            prompt: unescape(input),
            display: input.to_string(),
            command: None,
        };
    };

    // Reserved names are UI actions, not skills. A file named `model`
    // must not steal `/model` from the picker.
    if is_reserved_command(parsed.name) {
        return Expansion {
            prompt: input.to_string(),
            display: input.to_string(),
            command: None,
        };
    }

    if let Some(skill) = skills.command(parsed.name) {
        return Expansion {
            prompt: compose(&skill.body, parsed.rest),
            display: input.to_string(),
            command: Some(skill.name.clone()),
        };
    }

    if let Some(server) = lookup_mcp(mcp, parsed.name) {
        return Expansion {
            prompt: compose_mcp(&server.id, &server.tools, parsed.rest),
            display: input.to_string(),
            command: Some(server.id.clone()),
        };
    }

    Expansion {
        prompt: input.to_string(),
        display: input.to_string(),
        command: None,
    }
}

/// Expand `input` as if the skill `name` had been invoked on it.
///
/// This is how a *mode* applies a skill: Plan mode runs the `plan` skill over
/// whatever was typed, without anyone typing `/plan`. The whole message becomes
/// the argument, and `display` stays exactly what was typed — writing a `/plan`
/// prefix into the transcript would attribute words to the user they never
/// wrote.
///
/// A missing skill passes the message through untouched, for the same reason an
/// unknown `/token` does: absent config must not swallow what someone said.
pub fn expand_as(input: &str, skills: &SkillSet, name: &str) -> Expansion {
    let unescaped = unescape(input);
    let Some(skill) = skills.command(name) else {
        return Expansion {
            prompt: unescaped,
            display: input.to_string(),
            command: None,
        };
    };

    Expansion {
        prompt: compose(&skill.body, unescaped.trim()),
        display: input.to_string(),
        command: Some(skill.name.clone()),
    }
}

/// Put the skill body above the user's words, so the freshest instruction the
/// model reads is what was actually asked for.
fn compose(body: &str, rest: &str) -> String {
    let body = body.trim();
    if rest.is_empty() {
        body.to_string()
    } else {
        format!("{body}\n\n---\n\n{rest}")
    }
}

fn compose_mcp(id: &str, tools: &[String], rest: &str) -> String {
    let mut body = format!(
        "Use the `{id}` MCP server for this request. Call its tools rather than guessing or saying you cannot see it."
    );
    if !tools.is_empty() {
        let listed = tools
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ");
        body.push_str("\nTools: ");
        body.push_str(&listed);
    }
    compose(&body, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::parse_skill_markdown;
    use std::path::Path;

    fn skills_with(name: &str, body: &str) -> SkillSet {
        let mut set = SkillSet::default();
        let markdown =
            format!("---\nname: {name}\ndescription: does {name} things\n---\n\n{body}\n");
        let skill =
            parse_skill_markdown(&markdown, Path::new(&format!("/x/{name}/SKILL.md"))).unwrap();
        set.insert(skill);
        set
    }

    #[test]
    fn parses_a_bare_command() {
        let parsed = parse_command("/plan").unwrap();
        assert_eq!(parsed.name, "plan");
        assert_eq!(parsed.rest, "");
    }

    #[test]
    fn parses_a_command_with_arguments() {
        let parsed = parse_command("/plan add a health endpoint").unwrap();
        assert_eq!(parsed.name, "plan");
        assert_eq!(parsed.rest, "add a health endpoint");
    }

    #[test]
    fn leading_whitespace_is_tolerated() {
        assert_eq!(parse_command("   /plan x").unwrap().name, "plan");
    }

    #[test]
    fn a_slash_that_is_not_at_the_start_is_not_a_command() {
        assert_eq!(parse_command("look at /plan for me"), None);
        assert_eq!(parse_command("what about a/b"), None);
    }

    #[test]
    fn double_slash_escapes_to_a_literal() {
        assert_eq!(parse_command("//not-a-command"), None);
        assert_eq!(unescape("//usr/local/bin"), "/usr/local/bin");
        // Only the escape form is rewritten.
        assert_eq!(unescape("plain text"), "plain text");
    }

    #[test]
    fn a_bare_slash_is_not_a_command() {
        assert_eq!(parse_command("/"), None);
        assert_eq!(parse_command("/ "), None);
    }

    #[test]
    fn a_path_typed_first_is_left_alone() {
        // The single-slash path case cannot be distinguished from a command by
        // shape, so it resolves by lookup: no such skill, passed through.
        let skills = skills_with("plan", "Make a plan.");
        let out = expand("/etc/hosts is wrong", &skills, &[]);
        assert_eq!(out.prompt, "/etc/hosts is wrong");
        assert_eq!(out.command, None);
    }

    #[test]
    fn expands_a_known_command_into_the_skill_body() {
        let skills = skills_with("plan", "Research first, then write a plan.");
        let out = expand("/plan add auth", &skills, &[]);
        assert_eq!(out.command.as_deref(), Some("plan"));
        assert!(out.prompt.starts_with("Research first, then write a plan."));
        // The user's words come last so they are the freshest instruction.
        assert!(
            out.prompt.trim_end().ends_with("add auth"),
            "{}",
            out.prompt
        );
        // The transcript keeps what was typed, not the expansion.
        assert_eq!(out.display, "/plan add auth");
    }

    #[test]
    fn a_bare_command_expands_to_just_the_body() {
        let skills = skills_with("plan", "Research first.");
        let out = expand("/plan", &skills, &[]);
        assert_eq!(out.prompt, "Research first.");
        assert!(!out.prompt.contains("---"), "no empty argument separator");
    }

    #[test]
    fn an_unknown_command_is_sent_as_typed() {
        let skills = skills_with("plan", "body");
        let out = expand("/paln add auth", &skills, &[]);
        assert_eq!(
            out.prompt, "/paln add auth",
            "a typo must not eat the message"
        );
        assert_eq!(out.command, None);
    }

    #[test]
    fn a_mode_applies_a_skill_to_a_message_with_no_slash() {
        let skills = skills_with("plan", "Research first, then write a plan.");
        let out = expand_as("make this project better", &skills, "plan");
        assert_eq!(out.command.as_deref(), Some("plan"));
        assert!(out.prompt.starts_with("Research first, then write a plan."));
        assert!(out.prompt.trim_end().ends_with("make this project better"));
    }

    #[test]
    fn a_mode_does_not_rewrite_what_the_transcript_shows() {
        let skills = skills_with("plan", "body");
        let out = expand_as("make this project better", &skills, "plan");
        // The user did not type a slash, so the transcript must not grow one.
        assert_eq!(out.display, "make this project better");
    }

    #[test]
    fn a_mode_whose_skill_is_missing_sends_the_message_as_typed() {
        let skills = skills_with("other", "body");
        let out = expand_as("make this project better", &skills, "plan");
        assert_eq!(out.prompt, "make this project better");
        assert_eq!(out.command, None, "no skill means no document to frame");
    }

    #[test]
    fn a_mode_still_honours_the_escape() {
        let skills = skills_with("plan", "body");
        let out = expand_as("//usr/local/bin is wrong", &skills, "plan");
        assert!(
            out.prompt.trim_end().ends_with("/usr/local/bin is wrong"),
            "{}",
            out.prompt
        );
    }

    #[test]
    fn a_bare_message_under_a_mode_is_just_the_body() {
        let skills = skills_with("plan", "Research first.");
        let out = expand_as("   ", &skills, "plan");
        assert_eq!(out.prompt, "Research first.");
        assert!(!out.prompt.contains("---"), "no empty argument separator");
    }

    #[test]
    fn ordinary_messages_are_untouched() {
        let skills = skills_with("plan", "body");
        let out = expand("just fix the bug", &skills, &[]);
        assert_eq!(out.prompt, "just fix the bug");
        assert_eq!(out.display, "just fix the bug");
        assert_eq!(out.command, None);
    }

    fn haiku_mcp() -> McpSlash {
        McpSlash {
            id: "Haiku".into(),
            description: "Use the Haiku MCP server".into(),
            tools: vec!["manifest".into()],
        }
    }

    #[test]
    fn expands_an_mcp_server_into_a_use_this_server_prompt() {
        let skills = SkillSet::default();
        let out = expand("/haiku write a verse", &skills, &[haiku_mcp()]);
        assert_eq!(out.command.as_deref(), Some("Haiku"));
        assert_eq!(out.display, "/haiku write a verse");
        assert!(
            out.prompt.contains("Use the `Haiku` MCP server"),
            "{}",
            out.prompt
        );
        assert!(out.prompt.contains("`manifest`"), "{}", out.prompt);
        assert!(
            out.prompt.trim_end().ends_with("write a verse"),
            "{}",
            out.prompt
        );
    }

    #[test]
    fn a_bare_mcp_command_has_no_empty_separator() {
        let skills = SkillSet::default();
        let out = expand("/Haiku", &skills, &[haiku_mcp()]);
        assert_eq!(out.command.as_deref(), Some("Haiku"));
        assert!(!out.prompt.contains("---"), "{}", out.prompt);
    }

    #[test]
    fn a_skill_wins_when_an_mcp_server_shares_its_name() {
        let skills = skills_with("haiku", "Write a poem.");
        let out = expand("/haiku now", &skills, &[haiku_mcp()]);
        assert_eq!(out.command.as_deref(), Some("haiku"));
        assert!(out.prompt.starts_with("Write a poem."));
        assert!(!out.prompt.contains("MCP server"), "{}", out.prompt);
    }

    #[test]
    fn slash_list_skips_mcp_names_that_collide_with_skills() {
        let skills = skills_with("haiku", "Write a poem.");
        let listed = list_slash_commands(&skills, &[haiku_mcp()]);
        assert!(listed
            .iter()
            .any(|command| { command.name == "haiku" && command.kind == SlashKind::Skill }));
        assert!(!listed.iter().any(|command| command.kind == SlashKind::Mcp));
    }

    #[test]
    fn reserved_model_command_beats_a_skill_of_the_same_name() {
        let skills = skills_with("model", "Pick a model.");
        let listed = list_slash_commands(&skills, &[]);
        assert!(listed
            .iter()
            .any(|command| { command.name == "model" && command.kind == SlashKind::Builtin }));
        assert!(!listed
            .iter()
            .any(|command| { command.name == "model" && command.kind == SlashKind::Skill }));
        let out = expand("/model", &skills, &[]);
        assert_eq!(out.command, None);
        assert_eq!(out.prompt, "/model");
    }

    #[test]
    fn slash_list_includes_enabled_mcp_servers() {
        let listed = list_slash_commands(&SkillSet::default(), &[haiku_mcp()]);
        assert!(listed
            .iter()
            .any(|command| { command.name == "Haiku" && command.kind == SlashKind::Mcp }));
        assert!(listed
            .iter()
            .any(|command| { command.name == "model" && command.kind == SlashKind::Builtin }));
    }

    #[test]
    fn mcp_slashes_skip_disabled_servers() {
        use crate::config::McpServerConfig;
        use crate::mcp::McpCatalog;

        fn server(enabled: bool) -> McpServerConfig {
            McpServerConfig {
                command: "npx".into(),
                args: Vec::new(),
                url: None,
                headers: BTreeMap::new(),
                header_credentials: BTreeMap::new(),
                env_vars: Vec::new(),
                enabled,
                timeout_secs: 30,
            }
        }

        let mut servers = BTreeMap::new();
        servers.insert("Haiku".into(), server(true));
        servers.insert("off".into(), server(false));
        let listed = mcp_slashes(&servers, &McpCatalog::default());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "Haiku");
    }
}
