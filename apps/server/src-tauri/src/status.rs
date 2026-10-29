//! What the server window shows (ADR-0023): one full snapshot, rebuilt on every change
//! and sent as `server://status`. A missed event loses nothing — the next one is whole.

use oc_proto::{CampState, Channel, Role};
use serde::Serialize;

/// Where the camp is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Phase {
    /// Opening the database, starting LiveKit.
    Starting,
    /// Serving.
    Running,
    /// Did not start; `message` says why, in the server's words.
    Failed { message: String },
}

/// A member as the host sees them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub user_id: String,
    pub name: String,
    pub role: Role,
    /// Name of the campfire they sit at; `None` when not in voice.
    pub campfire: Option<String>,
    /// Its id, to move them elsewhere (ADR-0028).
    pub campfire_id: Option<String>,
    pub mic_muted: bool,
    /// Forbidden to speak until lifted (ADR-0028).
    pub silenced: bool,
    /// Hash of their avatar (ADR-0035); `None` without one.
    pub avatar: Option<String>,
}

/// The whole window, as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub phase: Phase,
    pub camp_name: String,
    /// Campfires and chats, in display order (ADR-0026).
    pub channels: Vec<Channel>,
    /// `host:port` friends connect to.
    pub address: Option<String>,
    pub fingerprint: Option<String>,
    pub join_link: Option<String>,
    /// Admins first, then moderators, then by name.
    pub members: Vec<Member>,
    /// The camp's own LiveKit; `None` while it restarts.
    pub livekit_pid: Option<u32>,
}

impl ServerStatus {
    pub fn starting() -> Self {
        Self {
            phase: Phase::Starting,
            camp_name: String::new(),
            channels: Vec::new(),
            address: None,
            fingerprint: None,
            join_link: None,
            members: Vec::new(),
            livekit_pid: None,
        }
    }

    pub fn failed(message: String) -> Self {
        Self {
            phase: Phase::Failed { message },
            ..Self::starting()
        }
    }
}

/// What does not change while the camp runs. The camp's name and channels do — they
/// come with every [`CampState`].
#[derive(Debug, Clone)]
pub struct Base {
    pub address: String,
    pub fingerprint: String,
    pub join_link: String,
}

impl Base {
    pub fn of(running: &oc_server::Running) -> Self {
        let join = &running.links.join;
        Self {
            address: format!("{}:{}", join.host, join.port),
            fingerprint: running.fingerprint.to_string(),
            join_link: join.to_string(),
        }
    }
}

/// The running camp's snapshot.
pub fn snapshot(base: &Base, state: &CampState, livekit_pid: Option<u32>) -> ServerStatus {
    let seat = |user_id: &str| {
        state.presence.iter().find_map(|channel| {
            let present = channel.members.iter().find(|m| m.user_id == user_id)?;
            let name = state
                .channels
                .iter()
                .find(|c| c.id == channel.channel_id)
                .map_or_else(|| channel.channel_id.clone(), |c| c.name.clone());
            Some((channel.channel_id.clone(), name, present.mic_muted))
        })
    };
    let mut members: Vec<Member> = state
        .members
        .iter()
        .map(|m| {
            let seat = seat(&m.user_id);
            Member {
                user_id: m.user_id.clone(),
                name: m.name.clone(),
                role: m.role,
                mic_muted: seat.as_ref().is_some_and(|(_, _, muted)| *muted),
                silenced: m.silenced,
                avatar: m.avatar.clone(),
                campfire_id: seat.as_ref().map(|(id, _, _)| id.clone()),
                campfire: seat.map(|(_, name, _)| name),
            }
        })
        .collect();
    members.sort_by(|a, b| {
        b.role
            .cmp(&a.role)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    ServerStatus {
        phase: Phase::Running,
        camp_name: state.name.clone(),
        channels: state.channels.clone(),
        address: Some(base.address.clone()),
        fingerprint: Some(base.fingerprint.clone()),
        join_link: Some(base.join_link.clone()),
        members,
        livekit_pid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_proto::{ChannelKind, ChannelPresence, MemberInfo, PresentMember};

    fn base() -> Base {
        Base {
            address: "192.168.1.2:7780".into(),
            fingerprint: "fp".into(),
            join_link: "ourcamp://join".into(),
        }
    }

    fn channel(id: &str, name: &str, kind: ChannelKind) -> Channel {
        Channel {
            id: id.into(),
            name: name.into(),
            kind,
            afk: false,
        }
    }

    fn member(id: &str, name: &str, role: Role) -> MemberInfo {
        MemberInfo {
            user_id: id.into(),
            name: name.into(),
            role,
            silenced: false,
            avatar: None,
        }
    }

    #[test]
    fn a_fresh_camp_shows_its_name_channels_and_link() {
        let state = CampState {
            name: "Мой сервер".into(),
            channels: vec![
                channel("c0", "Voice", ChannelKind::Voice),
                channel("c1", "Chat", ChannelKind::Text),
            ],
            ..CampState::default()
        };
        let status = snapshot(&base(), &state, Some(42));
        assert_eq!(status.phase, Phase::Running);
        assert_eq!(status.camp_name, "Мой сервер");
        assert_eq!(status.channels.len(), 2);
        assert_eq!(status.join_link.as_deref(), Some("ourcamp://join"));
        assert_eq!(status.members, []);
        assert_eq!(status.livekit_pid, Some(42));
    }

    #[test]
    fn members_show_their_campfire_by_name_by_role_then_name() {
        let state = CampState {
            name: "x".into(),
            channels: vec![
                channel("c0", "Лобби", ChannelKind::Voice),
                channel("c1", "Катка", ChannelKind::Voice),
            ],
            members: vec![
                member("u2", "лена", Role::Member),
                member("u4", "Боб", Role::Moderator),
                member("u3", "Артём", Role::Member),
                member("u1", "Дима", Role::Admin),
            ],
            presence: vec![ChannelPresence {
                channel_id: "c1".into(),
                members: vec![PresentMember {
                    user_id: "u2".into(),
                    mic_muted: true,
                }],
            }],
        };
        let status = snapshot(&base(), &state, None);
        let names: Vec<_> = status.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["Дима", "Боб", "Артём", "лена"],
            "admin, moderator, then by name in any case"
        );
        let lena = &status.members[3];
        assert_eq!(lena.campfire.as_deref(), Some("Катка"));
        assert_eq!(lena.campfire_id.as_deref(), Some("c1"));
        assert!(lena.mic_muted);
        assert_eq!(status.members[2].campfire, None, "not in voice");
    }
}
