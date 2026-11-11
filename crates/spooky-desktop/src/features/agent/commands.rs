//! The commands an agent's program offers, as the composer offers them after a slash.
//!
//! The names and descriptions come from that program and are drawn, never run: picking
//! one only writes it in the composer, and the user still sends it themselves.

use spooky_core::AgentCommand;

/// One command as the list offers it: `/security scan` is named `security scan`.
pub(crate) struct Choice<'a> {
    pub(crate) name: String,
    pub(crate) description: &'a str,
    /// What it takes after its name, as the program spells it; possibly empty.
    pub(crate) hint: &'a str,
}

/// Every command and subcommand, as one list.
pub(crate) fn choices(commands: &[AgentCommand]) -> Vec<Choice<'_>> {
    let mut all = Vec::new();
    for command in commands {
        all.push(Choice {
            name: command.name.clone(),
            description: &command.description,
            hint: &command.hint,
        });
        for sub in &command.subcommands {
            all.push(Choice {
                name: format!("{} {}", command.name, sub.name),
                description: &sub.description,
                hint: &sub.hint,
            });
        }
    }
    all
}

/// What the user is typing after a slash, or `None` when they are not writing a command.
/// Only a slash that starts the message is one, and only until the line ends.
pub(crate) fn typing(draft: &str) -> Option<&str> {
    let query = draft.strip_prefix('/')?;
    (!query.contains('\n')).then_some(query)
}

/// The commands `query` names, best first: the name starts with it, then the name holds
/// it, then only what it does mentions it.
pub(crate) fn matching<'a>(all: Vec<Choice<'a>>, query: &str) -> Vec<Choice<'a>> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return all;
    }
    let mut starts = Vec::new();
    let mut named = Vec::new();
    let mut described = Vec::new();
    for choice in all {
        let name = choice.name.to_lowercase();
        if name.starts_with(&needle) {
            starts.push(choice);
        } else if name.contains(&needle) {
            named.push(choice);
        } else if choice.description.to_lowercase().contains(&needle) {
            described.push(choice);
        }
    }
    starts.extend(named);
    starts.extend(described);
    starts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, description: &str, subs: &[&str]) -> AgentCommand {
        AgentCommand {
            name: name.into(),
            description: description.into(),
            hint: String::new(),
            subcommands: subs
                .iter()
                .map(|sub| AgentCommand {
                    name: (*sub).into(),
                    description: "does a thing".into(),
                    hint: String::new(),
                    subcommands: Vec::new(),
                })
                .collect(),
        }
    }

    fn offered() -> Vec<AgentCommand> {
        vec![
            command("model", "Show current model selection", &[]),
            command("security", "Run security scans", &["scan", "plan"]),
        ]
    }

    #[test]
    fn every_subcommand_is_offered_under_its_own_full_name() {
        let commands = offered();
        let names: Vec<_> = choices(&commands).into_iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            ["model", "security", "security scan", "security plan"]
        );
    }

    #[test]
    fn a_command_is_only_being_typed_when_the_slash_starts_the_message() {
        assert_eq!(typing("/mo"), Some("mo"));
        assert_eq!(typing("/"), Some(""));
        assert_eq!(typing("what /mo"), None);
        assert_eq!(typing(""), None);
        assert_eq!(typing("/security scan\nnow"), None, "that is its argument");
    }

    #[test]
    fn what_starts_with_the_query_comes_before_what_merely_holds_it() {
        let commands = offered();
        let names = |query: &str| -> Vec<String> {
            matching(choices(&commands), query)
                .into_iter()
                .map(|c| c.name)
                .collect()
        };
        assert_eq!(names("scan"), ["security scan", "security"]);
        assert_eq!(names("SEC"), ["security", "security scan", "security plan"]);
        assert!(names("zzz").is_empty());
        assert_eq!(names(" ").len(), 4, "blanks offer everything");
    }
}
