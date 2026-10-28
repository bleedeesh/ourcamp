//! What the server window shows (ADR-0023): one full snapshot, rebuilt on every change
//! and sent as `server://status`. A missed event loses nothing — the next one is whole.

use oc_proto::CampState;
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
    pub is_admin: bool,
    /// Name of the campfire they sit at; `None` when not in voice.
    pub campfire: Option<String>,
    pub mic_muted: bool,
}

/// The whole window, as data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub phase: Phase,
    pub camp_name: String,
    /// `host:port` friends connect to.
    pub address: Option<String>,
    pub fingerprint: Option<String>,
    pub join_link: Option<String>,
    /// The admin key link — only while nobody is admin yet (redeeming it makes one).
    pub claim_link: Option<String>,
    /// Admins first, then by name.
    pub members: Vec<Member>,
    /// The camp's own LiveKit; `None` while it restarts.
    pub livekit_pid: Option<u32>,
}

impl ServerStatus {
    pub fn starting() -> Self {
        Self {
            phase: Phase::Starting,
            camp_name: String::new(),
            address: None,
            fingerprint: None,
            join_link: None,
            claim_link: None,
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

/// What does not change while the camp runs.
#[derive(Debug, Clone)]
pub struct Base {
    pub camp: oc_server::store::Camp,
    pub address: String,
    pub fingerprint: String,
    pub join_link: String,
    pub claim_link: Option<String>,
}

impl Base {
    pub fn of(running: &oc_server::Running) -> Self {
        let join = &running.links.join;
        Self {
            camp: running.camp.clone(),
            address: format!("{}:{}", join.host, join.port),
            fingerprint: running.fingerprint.to_string(),
            join_link: join.to_string(),
            claim_link: running.links.claim.as_ref().map(ToString::to_string),
        }
    }
}

/// The running camp's snapshot.
pub fn snapshot(base: &Base, state: &CampState, livekit_pid: Option<u32>) -> ServerStatus {
    let seat = |user_id: &str| {
        state.presence.iter().find_map(|channel| {
            let present = channel.members.iter().find(|m| m.user_id == user_id)?;
            let name = base
                .camp
                .channels
                .iter()
                .find(|c| c.id == channel.channel_id)
                .map_or_else(|| channel.channel_id.clone(), |c| c.name.clone());
            Some((name, present.mic_muted))
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
                is_admin: m.is_admin,
                mic_muted: seat.as_ref().is_some_and(|(_, muted)| *muted),
                campfire: seat.map(|(name, _)| name),
            }
        })
        .collect();
    members.sort_by(|a, b| {
        b.is_admin
            .cmp(&a.is_admin)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let has_admin = members.iter().any(|m| m.is_admin);
    ServerStatus {
        phase: Phase::Running,
        camp_name: base.camp.name.clone(),
        address: Some(base.address.clone()),
        fingerprint: Some(base.fingerprint.clone()),
        join_link: Some(base.join_link.clone()),
        claim_link: if has_admin {
            None
        } else {
            base.claim_link.clone()
        },
        members,
        livekit_pid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oc_proto::{Channel, ChannelPresence, MemberInfo, PresentMember};

    fn base() -> Base {
        Base {
            camp: oc_server::store::Camp {
                name: "Вечерний кооп".into(),
                channels: vec![
                    Channel {
                        id: "c0".into(),
                        name: "Лобби".into(),
                        afk: false,
                    },
                    Channel {
                        id: "c1".into(),
                        name: "Катка".into(),
                        afk: false,
                    },
                ],
            },
            address: "192.168.1.2:7780".into(),
            fingerprint: "fp".into(),
            join_link: "ourcamp://join".into(),
            claim_link: Some("ourcamp://claim".into()),
        }
    }

    fn member(id: &str, name: &str, is_admin: bool) -> MemberInfo {
        MemberInfo {
            user_id: id.into(),
            name: name.into(),
            is_admin,
        }
    }

    #[test]
    fn a_fresh_camp_offers_the_admin_link() {
        let status = snapshot(&base(), &CampState::default(), Some(42));
        assert_eq!(status.phase, Phase::Running);
        assert_eq!(status.claim_link.as_deref(), Some("ourcamp://claim"));
        assert_eq!(status.members, []);
        assert_eq!(status.livekit_pid, Some(42));
    }

    #[test]
    fn members_show_their_campfire_by_name_admins_first() {
        let state = CampState {
            members: vec![
                member("u2", "лена", false),
                member("u3", "Артём", false),
                member("u1", "Дима", true),
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
            ["Дима", "Артём", "лена"],
            "admin, then by name, any case"
        );
        let lena = &status.members[2];
        assert_eq!(lena.campfire.as_deref(), Some("Катка"));
        assert!(lena.mic_muted);
        assert_eq!(status.members[1].campfire, None, "not in voice");
        assert_eq!(status.claim_link, None, "redeemed: there is an admin");
    }
}
