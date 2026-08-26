//! Slash commands, backed by skills.
//!
//! A command is not a new concept — it is a skill invoked by name. `/plan`
//! means "run a personal skill against what I typed next", so a new
//! command is a markdown file rather than a code change.
//!
//! Parsing is deliberately narrow. Only a token at the very start of the
//! message counts, and only `[a-z0-9-_]`, because everything else people type
//! at the start of a line — a path, a regex, a URL — must survive untouched.

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

/// Expand a leading command against the available skills.
///
/// An unrecognised `/token` is passed through unchanged rather than rejected: a
/// typo should not swallow the message, and the model can say it did not
/// understand far more usefully than an error dialog can.
pub fn expand(input: &str, skills: &SkillSet) -> Expansion {
    let Some(parsed) = parse_command(input) else {
        return Expansion {
            prompt: unescape(input),
            display: input.to_string(),
            command: None,
        };
    };

    let Some(skill) = skills.command(parsed.name) else {
        return Expansion {
            prompt: input.to_string(),
            display: input.to_string(),
            command: None,
        };
    };

    Expansion {
        prompt: compose(&skill.body, parsed.rest),
        display: input.to_string(),
        command: Some(skill.name.clone()),
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
        let out = expand("/etc/hosts is wrong", &skills);
        assert_eq!(out.prompt, "/etc/hosts is wrong");
        assert_eq!(out.command, None);
    }

    #[test]
    fn expands_a_known_command_into_the_skill_body() {
        let skills = skills_with("plan", "Research first, then write a plan.");
        let out = expand("/plan add auth", &skills);
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
        let out = expand("/plan", &skills);
        assert_eq!(out.prompt, "Research first.");
        assert!(!out.prompt.contains("---"), "no empty argument separator");
    }

    #[test]
    fn an_unknown_command_is_sent_as_typed() {
        let skills = skills_with("plan", "body");
        let out = expand("/paln add auth", &skills);
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
        let out = expand("just fix the bug", &skills);
        assert_eq!(out.prompt, "just fix the bug");
        assert_eq!(out.display, "just fix the bug");
        assert_eq!(out.command, None);
    }
}
