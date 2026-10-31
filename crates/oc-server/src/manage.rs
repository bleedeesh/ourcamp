//! What the owner of a camp changes (ADR-0026): its name, roles, channels. The server
//! window calls it in-process; admins and moderators will reach the same code through
//! the API, within their rights.
//!
//! Every change is written to the database first, then the presence loop is asked for
//! a snapshot right away (`refresh`), so members see it within a moment, not a poll.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use oc_proto::{AvatarFormat, CampEvent, Channel, ChannelKind, Role};
use tokio::sync::broadcast;

use crate::presence::Refresh;
use crate::rooms::{RoomError, RoomService};
use crate::store::{Author, Deleted, Posted, Store, StoreError};

/// Longest camp name, in characters.
pub const MAX_CAMP_NAME: usize = 48;
/// Longest channel name, in characters.
pub const MAX_CHANNEL_NAME: usize = 32;

/// Why a change was refused or failed.
#[derive(Debug)]
pub enum ManageError {
    /// No such member or channel.
    NotFound,
    /// Empty, or longer than the limit (the limit is in the variant).
    BadName(usize),
    /// The AFK campfire stays (ADR-0025).
    AfkIsPermanent,
    /// The database refused.
    Store(StoreError),
    /// The channel is gone, but LiveKit did not close its room.
    RoomNotClosed(RoomError),
    /// Moving someone who is not in voice right now.
    NotInVoice,
    /// Moving someone into a chat: only campfires have voice.
    NotACampfire,
    /// LiveKit refused the voice part of the action.
    LiveKit(RoomError),
}

impl fmt::Display for ManageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such member or channel"),
            Self::BadName(max) => write!(f, "a name is 1 to {max} characters"),
            Self::AfkIsPermanent => write!(f, "the AFK campfire cannot be removed"),
            Self::Store(e) => write!(f, "{e}"),
            Self::RoomNotClosed(e) => write!(f, "channel removed, its room was not: {e}"),
            Self::NotInVoice => write!(f, "the member is not in voice"),
            Self::NotACampfire => write!(f, "only campfires have voice"),
            Self::LiveKit(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ManageError {}

impl From<StoreError> for ManageError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// A trimmed name within `max` characters.
fn name(raw: &str, max: usize) -> Result<&str, ManageError> {
    let name = raw.trim();
    let chars = name.chars().count();
    if chars == 0 || chars > max {
        return Err(ManageError::BadName(max));
    }
    Ok(name)
}

/// An event for `/api/v1/events` streams beside the snapshots: one member's (ADR-0028),
/// or everyone's (a chat message, ADR-0044).
#[derive(Debug, Clone)]
pub(crate) struct Directive {
    pub to: Audience,
    pub event: CampEvent,
}

/// Whose streams a [`Directive`] goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Audience {
    /// This member's.
    Member(String),
    /// Every member's.
    Everyone,
}

impl Audience {
    /// Whether `user_id`'s stream gets it.
    pub(crate) fn includes(&self, user_id: &str) -> bool {
        match self {
            Self::Member(id) => id == user_id,
            Self::Everyone => true,
        }
    }
}

/// The owner's handle on a running camp. Cheap to clone.
#[derive(Clone)]
pub struct Manager {
    store: Arc<Mutex<Store>>,
    rooms: Arc<RoomService>,
    refresh: Arc<Refresh>,
    directives: broadcast::Sender<Directive>,
}

impl Manager {
    pub(crate) fn new(
        store: Arc<Mutex<Store>>,
        rooms: Arc<RoomService>,
        refresh: Arc<Refresh>,
        directives: broadcast::Sender<Directive>,
    ) -> Self {
        Self {
            store,
            rooms,
            refresh,
            directives,
        }
    }

    /// Runs a store call off the async workers, then asks for a fresh snapshot.
    async fn change<R: Send + 'static>(
        &self,
        call: impl FnOnce(&mut Store) -> Result<R, ManageError> + Send + 'static,
    ) -> Result<R, ManageError> {
        let store = Arc::clone(&self.store);
        let result = tokio::task::spawn_blocking(move || {
            let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
            call(&mut store)
        })
        .await
        .expect("store task panicked")?;
        self.refresh.ask();
        Ok(result)
    }

    /// Waits until the camp's snapshot holds every change made so far (ADR-0041): the
    /// owner's next command sees it. Unbounded if the camp is stopping — add a timeout.
    pub async fn settled(&self) {
        self.refresh.settled().await;
    }

    /// Removes a message, if `actor` may (ADR-0048), and tells everyone.
    ///
    /// # Errors
    /// [`ManageError::Store`].
    pub async fn delete_message(
        &self,
        channel_id: &str,
        seq: u64,
        actor_id: &str,
        actor_role: Role,
    ) -> Result<Deleted, ManageError> {
        let (chat, actor) = (channel_id.to_owned(), actor_id.to_owned());
        let deleted = self
            .change(move |s| Ok(s.delete_message(&chat, seq, &actor, actor_role)?))
            .await?;
        if deleted == Deleted::Done {
            let _ = self.directives.send(Directive {
                to: Audience::Everyone,
                event: CampEvent::MessageDeleted {
                    channel_id: channel_id.to_owned(),
                    seq,
                },
            });
        }
        Ok(deleted)
    }

    /// Opens the camp to anyone with its address, or back to invites only (ADR-0040).
    ///
    /// # Errors
    /// [`ManageError::Store`].
    pub async fn set_open(&self, open: bool) -> Result<(), ManageError> {
        self.change(move |s| Ok(s.set_open(open)?)).await
    }

    /// Renames the camp.
    ///
    /// # Errors
    /// [`ManageError::BadName`], [`ManageError::Store`].
    pub async fn rename_camp(&self, new_name: &str) -> Result<(), ManageError> {
        let new_name = name(new_name, MAX_CAMP_NAME)?.to_owned();
        self.change(move |s| Ok(s.rename_camp(&new_name)?)).await
    }

    /// Gives a member a role (the owner may give any, ADR-0026).
    ///
    /// # Errors
    /// [`ManageError::NotFound`], [`ManageError::Store`].
    pub async fn set_role(&self, user_id: &str, role: Role) -> Result<(), ManageError> {
        let user_id = user_id.to_owned();
        self.change(move |s| {
            if s.set_role(&user_id, role)? {
                Ok(())
            } else {
                Err(ManageError::NotFound)
            }
        })
        .await
    }

    /// Adds a campfire or a chat, before the AFK campfire.
    ///
    /// # Errors
    /// [`ManageError::BadName`], [`ManageError::Store`].
    pub async fn create_channel(
        &self,
        new_name: &str,
        kind: ChannelKind,
    ) -> Result<Channel, ManageError> {
        let new_name = name(new_name, MAX_CHANNEL_NAME)?.to_owned();
        self.change(move |s| Ok(s.create_channel(&new_name, kind)?))
            .await
    }

    /// Renames a channel (the AFK campfire too).
    ///
    /// # Errors
    /// [`ManageError::BadName`], [`ManageError::NotFound`], [`ManageError::Store`].
    pub async fn rename_channel(&self, id: &str, new_name: &str) -> Result<(), ManageError> {
        let new_name = name(new_name, MAX_CHANNEL_NAME)?.to_owned();
        let id = id.to_owned();
        self.change(move |s| {
            if s.rename_channel(&id, &new_name)? {
                Ok(())
            } else {
                Err(ManageError::NotFound)
            }
        })
        .await
    }

    /// Removes a channel. For a campfire, LiveKit closes its room: whoever sits there is
    /// disconnected and their app says the campfire was removed. LiveKit being down is
    /// fine — then nobody can be sitting there.
    ///
    /// # Errors
    /// [`ManageError::NotFound`], [`ManageError::AfkIsPermanent`], [`ManageError::Store`],
    /// [`ManageError::LiveKit`] (the channel is gone either way).
    pub async fn delete_channel(&self, id: &str) -> Result<(), ManageError> {
        let id = id.to_owned();
        let removed = self
            .change(move |s| match s.channel(&id)? {
                None => Err(ManageError::NotFound),
                Some(channel) if channel.afk => Err(ManageError::AfkIsPermanent),
                Some(_) => s.delete_channel(&id)?.ok_or(ManageError::NotFound),
            })
            .await?;
        if removed.kind == ChannelKind::Voice {
            match self.rooms.delete_room(&removed.id).await {
                Ok(()) | Err(RoomError::Unreachable(_)) => {}
                Err(e) => return Err(ManageError::RoomNotClosed(e)),
            }
        }
        Ok(())
    }

    /// Runs a store read off the async workers (no snapshot: nothing changed).
    async fn read<R: Send + 'static>(
        &self,
        call: impl FnOnce(&Store) -> Result<R, ManageError> + Send + 'static,
    ) -> Result<R, ManageError> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let store = store.lock().unwrap_or_else(PoisonError::into_inner);
            call(&store)
        })
        .await
        .expect("store task panicked")
    }

    /// The room `user_id` sits in now; `None` when not in voice — or when LiveKit is
    /// down, which means the same: nobody can be in voice then.
    async fn seat(&self, user_id: &str) -> Result<Option<String>, ManageError> {
        match self.rooms.presence().await {
            Ok(rooms) => Ok(rooms
                .into_iter()
                .find(|(_, people)| people.iter().any(|p| p.identity == user_id))
                .map(|(room, _)| room)),
            Err(RoomError::Unreachable(_)) => Ok(None),
            Err(e) => Err(ManageError::LiveKit(e)),
        }
    }

    /// Sends `event` to `user_id`'s event stream. Nobody listening is fine: they are
    /// offline, and their next connect reflects the change anyway.
    fn tell(&self, user_id: &str, event: CampEvent) {
        let _ = self.directives.send(Directive {
            to: Audience::Member(user_id.to_owned()),
            event,
        });
    }

    /// Keeps a message in a chat and sends it to everyone (ADR-0044); the snapshot's
    /// `chats` follows, for apps that missed the event. `message` was checked by the
    /// caller (length, photo sizes).
    ///
    /// # Errors
    /// [`ManageError::Store`].
    pub async fn post_message(
        &self,
        channel_id: &str,
        author: Author<'_>,
        message: oc_proto::NewMessage,
    ) -> Result<Posted, ManageError> {
        let (channel_id, user_id, name) = (
            channel_id.to_owned(),
            author.user_id.to_owned(),
            author.name.to_owned(),
        );
        let posted = self
            .change(move |s| {
                let author = Author {
                    user_id: &user_id,
                    name: &name,
                };
                Ok(s.post_message(&channel_id, author, &message.text, message.photo.as_ref())?)
            })
            .await?;
        if let Posted::Message(sent) = &posted {
            let _ = self.directives.send(Directive {
                to: Audience::Everyone,
                event: CampEvent::Message(sent.clone()),
            });
        }
        Ok(posted)
    }

    /// Removes a member (a kick, not a ban — the friends link brings them back): their
    /// app is told first and leaves voice and the camp itself; LiveKit then takes them
    /// out of the room regardless (ADR-0028).
    ///
    /// # Errors
    /// [`ManageError::NotFound`], [`ManageError::Store`], [`ManageError::LiveKit`] (they
    /// are no member any more either way).
    pub async fn kick(&self, user_id: &str) -> Result<(), ManageError> {
        let id = user_id.to_owned();
        self.change(move |s| {
            if s.remove_member(&id)? {
                Ok(())
            } else {
                Err(ManageError::NotFound)
            }
        })
        .await?;
        self.tell(user_id, CampEvent::Kicked);
        if let Some(room) = self.seat(user_id).await? {
            self.rooms
                .remove_participant(&room, user_id)
                .await
                .map_err(ManageError::LiveKit)?;
        }
        Ok(())
    }

    /// Moves a member who is in voice to another campfire: their app joins it (ADR-0028).
    ///
    /// # Errors
    /// [`ManageError::NotFound`] (member or channel), [`ManageError::NotACampfire`],
    /// [`ManageError::NotInVoice`], [`ManageError::LiveKit`].
    pub async fn move_member(&self, user_id: &str, channel_id: &str) -> Result<(), ManageError> {
        let (id, channel) = (user_id.to_owned(), channel_id.to_owned());
        self.read(move |s| {
            let member = s.members()?.into_iter().any(|m| m.user_id == id);
            match s.channel(&channel)? {
                _ if !member => Err(ManageError::NotFound),
                None => Err(ManageError::NotFound),
                Some(c) if c.kind != ChannelKind::Voice => Err(ManageError::NotACampfire),
                Some(_) => Ok(()),
            }
        })
        .await?;
        match self.seat(user_id).await? {
            None => Err(ManageError::NotInVoice),
            Some(room) if room == channel_id => Ok(()),
            Some(_) => {
                self.tell(
                    user_id,
                    CampEvent::Moved {
                        channel_id: channel_id.to_owned(),
                    },
                );
                Ok(())
            }
        }
    }

    /// Keeps a member's avatar (ADR-0035); everyone's snapshot gets its new hash. `bytes`
    /// are checked by the caller (size, [`AvatarFormat::sniff`]).
    ///
    /// # Errors
    /// [`ManageError::NotFound`], [`ManageError::Store`].
    pub async fn set_avatar(
        &self,
        user_id: &str,
        format: AvatarFormat,
        bytes: Vec<u8>,
    ) -> Result<(), ManageError> {
        let id = user_id.to_owned();
        self.change(move |s| {
            if s.set_avatar(&id, format, &bytes)? {
                Ok(())
            } else {
                Err(ManageError::NotFound)
            }
        })
        .await
    }

    /// Renames a member: everyone sees it in the next snapshot (ADR-0057).
    ///
    /// # Errors
    /// [`ManageError::NotFound`], [`ManageError::Store`].
    pub async fn set_member_name(&self, user_id: &str, name: String) -> Result<(), ManageError> {
        let id = user_id.to_owned();
        self.change(move |s| {
            if s.set_member_name(&id, &name)? {
                Ok(())
            } else {
                Err(ManageError::NotFound)
            }
        })
        .await
    }

    /// A member's avatar: media type and bytes (the owner's window shows it).
    ///
    /// # Errors
    /// [`ManageError::Store`].
    pub async fn avatar(&self, user_id: &str) -> Result<Option<(String, Vec<u8>)>, ManageError> {
        let id = user_id.to_owned();
        self.read(move |s| Ok(s.avatar(&id)?)).await
    }

    /// Drops a member's avatar: they show their initial again.
    ///
    /// # Errors
    /// [`ManageError::Store`].
    pub async fn clear_avatar(&self, user_id: &str) -> Result<(), ManageError> {
        let id = user_id.to_owned();
        self.change(move |s| Ok(s.clear_avatar(&id)?)).await
    }

    /// Forbids a member to speak until lifted, or lifts it. It holds across reconnects:
    /// room tokens come without publishing; if they sit in a room now, LiveKit takes
    /// their microphone down (or allows it back) at once (ADR-0028).
    ///
    /// # Errors
    /// [`ManageError::NotFound`], [`ManageError::Store`], [`ManageError::LiveKit`] (the
    /// flag is saved either way and applies from their next join).
    pub async fn set_silenced(&self, user_id: &str, silenced: bool) -> Result<(), ManageError> {
        let id = user_id.to_owned();
        self.change(move |s| {
            if s.set_silenced(&id, silenced)? {
                Ok(())
            } else {
                Err(ManageError::NotFound)
            }
        })
        .await?;
        if let Some(room) = self.seat(user_id).await? {
            self.rooms
                .set_can_publish(&room, user_id, !silenced)
                .await
                .map_err(ManageError::LiveKit)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_bounded_in_characters_not_bytes() {
        assert_eq!(name("  Кооп  ", 48).unwrap(), "Кооп");
        assert!(matches!(name("   ", 48), Err(ManageError::BadName(48))));
        assert_eq!(name(&"я".repeat(32), 32).unwrap().chars().count(), 32);
        assert!(matches!(
            name(&"я".repeat(33), 32),
            Err(ManageError::BadName(32))
        ));
    }
}
