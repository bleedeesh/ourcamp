//! The owner's commands of `oc-server` (ADR-0041): `oc-server [--data DIR] <command>`.
//! They reach the camp running in that data directory through its control
//! (`oc_server::control`), so they act at once — as from the server window.

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use oc_proto::{CampState, ChannelKind, Role};
use oc_server::control::{role_name, Command, ControlClient};

pub const HELP: &str = "\
Owner's commands — to the camp running in --data DIR:
  status                          name, link, channels, people
  role WHO member|moderator|admin give a role
  rename NAME                     rename the camp
  channel add NAME [--text]       add a voice channel (a chat with --text)
  channel rename CHANNEL NAME     rename a channel
  channel remove CHANNEL          remove a channel (those in it leave voice)
  kick WHO                        kick out (no ban: the friends link brings them back)
  silence WHO / unsilence WHO     forbid speaking / allow it again
  move WHO CHANNEL                move someone in voice to a voice channel
  open / invite-only              anyone with the address may come in / invites only
WHO and CHANNEL: a name (any case) or an id from `status`.
";

/// What the owner asked for.
#[derive(Debug, PartialEq, Eq)]
pub enum Owner {
    Status,
    Do(Command),
}

fn role(word: &str) -> Result<Role, String> {
    match word.to_lowercase().as_str() {
        "member" | "участник" => Ok(Role::Member),
        "moderator" | "модератор" => Ok(Role::Moderator),
        "admin" | "админ" => Ok(Role::Admin),
        other => Err(format!("role {other}: member, moderator or admin")),
    }
}

/// The rest of the words as one name: `rename Вечерний кооп` needs no quotes.
fn rest(words: &[String]) -> Option<String> {
    (!words.is_empty()).then(|| words.join(" "))
}

/// Reads a command; `Ok(None)`: not a command word at all (the server is to run).
///
/// # Errors
/// A command word with wrong arguments: what is missing.
pub fn parse(words: &[String]) -> Result<Option<Owner>, String> {
    let need = |what: &str| format!("{} needs {what} (see --help)", words[0]);
    let one = |i: usize, what: &str| words.get(i).cloned().ok_or_else(|| need(what));
    let command = match words.first().map(String::as_str) {
        Some("status") => return Ok(Some(Owner::Status)),
        Some("role") => Command::Role {
            who: one(1, "WHO")?,
            role: role(&one(2, "a role")?)?,
        },
        Some("rename") => Command::Rename {
            name: rest(&words[1..]).ok_or_else(|| need("NAME"))?,
        },
        Some("channel") => match words.get(1).map(String::as_str) {
            Some("add") => {
                let text = words.iter().any(|w| w == "--text");
                let name: Vec<_> = words[2..]
                    .iter()
                    .filter(|w| *w != "--text")
                    .cloned()
                    .collect();
                Command::AddChannel {
                    name: rest(&name).ok_or_else(|| need("NAME"))?,
                    text,
                }
            }
            Some("rename") => Command::RenameChannel {
                channel: one(2, "CHANNEL")?,
                name: rest(words.get(3..).unwrap_or_default()).ok_or_else(|| need("NAME"))?,
            },
            Some("remove") => Command::RemoveChannel {
                channel: one(2, "CHANNEL")?,
            },
            _ => return Err(need("add, rename or remove")),
        },
        Some("kick") => Command::Kick {
            who: one(1, "WHO")?,
        },
        Some("silence") => Command::Silence {
            who: one(1, "WHO")?,
            silenced: true,
        },
        Some("unsilence") => Command::Silence {
            who: one(1, "WHO")?,
            silenced: false,
        },
        Some("move") => Command::Move {
            who: one(1, "WHO")?,
            channel: rest(words.get(2..).unwrap_or_default()).ok_or_else(|| need("CHANNEL"))?,
        },
        Some("open") => Command::Open { open: true },
        Some("invite-only") => Command::Open { open: false },
        _ => return Ok(None),
    };
    Ok(Some(Owner::Do(command)))
}

/// The camp as text for the terminal.
fn describe(state: &CampState, link: &str) -> String {
    let mut out = format!(
        "«{}» · {}\nСсылка: {link}\n\nКаналы:\n",
        state.name,
        if state.open {
            "открытый"
        } else {
            "по приглашению"
        }
    );
    for c in &state.channels {
        let kind = match (c.kind, c.afk) {
            (_, true) => "AFK",
            (ChannelKind::Text, _) => "чат",
            (ChannelKind::Voice, _) => "голос",
        };
        // Writing into a String cannot fail.
        let _ = writeln!(out, "  {:<24} {:<6} {}", c.name, kind, c.id);
    }
    let _ = writeln!(out, "\nЛюди ({}):", state.members.len());
    for m in &state.members {
        let seat = state
            .presence
            .iter()
            .find(|p| p.members.iter().any(|x| x.user_id == m.user_id))
            .and_then(|p| state.channels.iter().find(|c| c.id == p.channel_id))
            .map_or_else(|| "—".to_owned(), |c| format!("в «{}»", c.name));
        let silenced = if m.silenced {
            "  говорить запрещено"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  {:<20} {:<10} {:<18} {}{silenced}",
            m.name,
            role_name(m.role),
            seat,
            m.user_id
        );
    }
    out
}

/// Runs `owner` against the camp in `data_dir`.
pub async fn run(data_dir: &Path, owner: Owner) -> ExitCode {
    let result = match ControlClient::find(data_dir) {
        Ok(client) => match owner {
            Owner::Status => client.status().await.map(|s| describe(&s.state, &s.link)),
            Owner::Do(command) => client.send(&command).await.map(|text| format!("{text}\n")),
        },
        Err(e) => Err(e),
    };
    match result {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("oc-server: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    fn command(line: &str) -> Command {
        match parse(&words(line)).unwrap().unwrap() {
            Owner::Do(c) => c,
            Owner::Status => panic!("status"),
        }
    }

    #[test]
    fn commands_read_like_sentences() {
        assert_eq!(parse(&words("status")).unwrap(), Some(Owner::Status));
        assert_eq!(
            command("role Лена админ"),
            Command::Role {
                who: "Лена".into(),
                role: Role::Admin
            }
        );
        assert_eq!(
            command("rename Вечерний кооп"),
            Command::Rename {
                name: "Вечерний кооп".into()
            }
        );
        assert_eq!(
            command("channel add Чат команды --text"),
            Command::AddChannel {
                name: "Чат команды".into(),
                text: true
            }
        );
        assert_eq!(
            command("channel rename c3 Тихий час"),
            Command::RenameChannel {
                channel: "c3".into(),
                name: "Тихий час".into()
            }
        );
        assert_eq!(
            command("silence Боб"),
            Command::Silence {
                who: "Боб".into(),
                silenced: true
            }
        );
        assert_eq!(
            command("move Боб AFK"),
            Command::Move {
                who: "Боб".into(),
                channel: "AFK".into()
            }
        );
        assert_eq!(command("invite-only"), Command::Open { open: false });
    }

    #[test]
    fn missing_words_are_said_and_others_are_not_commands() {
        assert!(parse(&words("role Лена")).unwrap_err().contains("a role"));
        assert!(parse(&words("role Лена king"))
            .unwrap_err()
            .contains("member, moderator or admin"));
        assert!(parse(&words("channel"))
            .unwrap_err()
            .contains("add, rename or remove"));
        assert!(parse(&words("kick")).unwrap_err().contains("WHO"));
        assert_eq!(parse(&words("serve")).unwrap(), None, "not a command word");
        assert_eq!(parse(&[]).unwrap(), None);
    }
}
