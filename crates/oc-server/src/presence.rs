//! Who sits at which campfire (ADR-0021): the server asks LiveKit over loopback once a
//! second and publishes a [`CampState`] snapshot whenever it changes. `/api/v1/events`
//! streams the snapshots to clients.
//!
//! Polling instead of LiveKit webhooks (ADR-0004): webhooks would have to reach this
//! server over TLS LiveKit does not trust (self-signed), and from inside Docker in
//! development; a 1 s poll of a local API costs nothing at camp scale.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use oc_proto::{CampState, ChannelKind, ChannelPresence, PresentMember};
use tokio::sync::{watch, Notify};

use crate::rooms::{Participant, RoomService};
use crate::store::{Store, StoreResult};

/// How often LiveKit is asked. Joins and leaves show up within this.
const POLL: Duration = Duration::from_secs(1);

/// Asks the loop for a snapshot now, after the owner changed the camp (ADR-0026), and
/// lets the owner's commands wait until one with their change is out (ADR-0041).
pub struct Refresh {
    wake: Notify,
    /// Changes asked about so far.
    asked: AtomicU64,
    /// How many of them the last published snapshot already holds.
    published: watch::Sender<u64>,
}

impl Refresh {
    pub fn new() -> Self {
        Self {
            wake: Notify::new(),
            asked: AtomicU64::new(0),
            published: watch::Sender::new(0),
        }
    }

    /// After a change in the store: a snapshot now instead of at the next tick.
    pub fn ask(&self) {
        self.asked.fetch_add(1, Ordering::SeqCst);
        // `notify_one` keeps a permit if the loop is mid-round: the change is never lost.
        self.wake.notify_one();
    }

    /// Waits for a snapshot built after every change asked about so far. Never returns
    /// if the loop has stopped: callers bound it with a timeout.
    pub async fn settled(&self) {
        let wanted = self.asked.load(Ordering::SeqCst);
        // The sender lives in `self`, so the wait cannot fail on a closed channel.
        let _ = self.published.subscribe().wait_for(|&n| n >= wanted).await;
    }
}

/// The snapshot to start with: members from the database, nobody at any campfire yet.
/// Published before LiveKit is first asked, so members show up even while it is slow.
pub fn initial(store: &Store) -> StoreResult<CampState> {
    snapshot(store, &HashMap::new())
}

/// Builds the snapshot clients see: the camp and its members from the database, presence
/// from LiveKit restricted to this camp's campfires and members, in channel order.
fn snapshot(store: &Store, rooms: &HashMap<String, Vec<Participant>>) -> StoreResult<CampState> {
    let members = store.members()?;
    let known: std::collections::HashSet<&str> =
        members.iter().map(|m| m.user_id.as_str()).collect();
    let camp = store.camp()?;
    let presence = camp
        .channels
        .iter()
        .filter(|channel| channel.kind == ChannelKind::Voice)
        .filter_map(|channel| {
            let people: BTreeMap<&str, &Participant> = rooms
                .get(&channel.id)?
                .iter()
                .filter(|p| known.contains(p.identity.as_str()))
                .map(|p| (p.identity.as_str(), p))
                .collect();
            (!people.is_empty()).then(|| ChannelPresence {
                channel_id: channel.id.clone(),
                members: people
                    .into_iter()
                    .map(|(user_id, p)| PresentMember {
                        user_id: user_id.to_owned(),
                        mic_muted: p.mic_muted(),
                        streaming: p.streaming(),
                    })
                    .collect(),
            })
        })
        .collect();
    Ok(CampState {
        name: camp.name,
        channels: camp.channels,
        members,
        presence,
        open: camp.open,
        chats: store.latest_messages()?,
    })
}

/// Polls until `shutdown` turns true, publishing changes to `out`. `refresh` asks for a
/// round now instead of at the next tick.
pub async fn run(
    store: Arc<Mutex<Store>>,
    api: Arc<RoomService>,
    out: watch::Sender<Arc<CampState>>,
    refresh: Arc<Refresh>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut livekit_down = false;
    // Who sat where at the last answer: a refresh publishes the camp's change at once
    // with it, without waiting for LiveKit (up to `CALL_TIMEOUT` when it is slow).
    let mut rooms: Arc<HashMap<String, Vec<Participant>>> = Arc::default();
    loop {
        let refreshed = tokio::select! {
            _ = tick.tick() => false,
            () = refresh.wake.notified() => true,
            _ = shutdown.changed() => return,
        };
        if refreshed {
            publish(&store, &rooms, &out, &refresh).await;
        }
        // A round can take up to `CALL_TIMEOUT` per call; stopping must not wait for it.
        let polled = tokio::select! {
            polled = api.presence() => polled,
            _ = shutdown.changed() => return,
        };
        rooms = Arc::new(match polled {
            Ok(rooms) => {
                if livekit_down {
                    eprintln!("[oc-server] LiveKit reachable again");
                    livekit_down = false;
                }
                rooms
            }
            Err(e) => {
                // Logged once per outage, not every second; campfires show as empty.
                if !livekit_down {
                    eprintln!("[oc-server] presence unavailable: {e}");
                    livekit_down = true;
                }
                HashMap::new()
            }
        });
        publish(&store, &rooms, &out, &refresh).await;
    }
}

/// Builds the snapshot (SQLite: off the async workers) and publishes it if it changed.
async fn publish(
    store: &Arc<Mutex<Store>>,
    rooms: &Arc<HashMap<String, Vec<Participant>>>,
    out: &watch::Sender<Arc<CampState>>,
    refresh: &Refresh,
) {
    // Read before the store is: every change counted here is in the snapshot built below
    // (the change is written before it is counted, both under the same store lock).
    let asked = refresh.asked.load(Ordering::SeqCst);
    let (store, rooms) = (Arc::clone(store), Arc::clone(rooms));
    let built = tokio::task::spawn_blocking(move || {
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        snapshot(&store, &rooms)
    })
    .await;
    match built {
        Ok(Ok(state)) => {
            // `send_if_modified`: subscribers wake only on a real change.
            out.send_if_modified(|current| {
                let changed = **current != state;
                if changed {
                    *current = Arc::new(state);
                }
                changed
            });
            refresh.published.send_if_modified(|published| {
                let newer = asked > *published;
                if newer {
                    *published = asked;
                }
                newer
            });
        }
        Ok(Err(e)) => eprintln!("[oc-server] presence snapshot: {e}"),
        Err(_) => eprintln!("[oc-server] presence snapshot task panicked"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Device;

    fn participant(json: &str) -> Participant {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn mic_state_comes_from_the_microphone_track_as_livekit_reports_it() {
        // Trimmed from a real `ListParticipants` answer of livekit-server 1.13.
        let muted = participant(
            r#"{"sid":"PA_1","identity":"u1","state":"ACTIVE","tracks":[{"sid":"TR_1","type":"AUDIO",
            "name":"microphone","muted":true,"source":"MICROPHONE","mime_type":"audio/red"}],"name":"Проба"}"#,
        );
        assert!(muted.mic_muted());
        let talking = participant(
            r#"{"identity":"u2","tracks":[{"type":"AUDIO","source":"MICROPHONE","muted":false}]}"#,
        );
        assert!(!talking.mic_muted());
        let screen_only = participant(
            r#"{"identity":"u3","tracks":[{"type":"AUDIO","source":"SCREEN_SHARE_AUDIO","muted":false}]}"#,
        );
        assert!(screen_only.mic_muted(), "stream audio is not a microphone");
        assert!(
            participant(r#"{"identity":"u4"}"#).mic_muted(),
            "no tracks at all"
        );
    }

    #[test]
    fn streaming_means_a_screen_track_as_livekit_reports_it() {
        let showing = participant(
            r#"{"identity":"u1","tracks":[{"sid":"TR_1","type":"VIDEO","name":"screen",
            "source":"CAMERA","width":1280,"height":720}]}"#,
        );
        assert!(showing.streaming());
        let camera = participant(
            r#"{"identity":"u2","tracks":[{"type":"VIDEO","source":"CAMERA","muted":false}]}"#,
        );
        assert!(!camera.streaming(), "a camera is not a stream");
        let sound_only = participant(
            r#"{"identity":"u3","tracks":[{"type":"AUDIO","source":"SCREEN_SHARE_AUDIO"}]}"#,
        );
        assert!(!sound_only.streaming(), "stream audio without the picture");
        let paused = participant(
            r#"{"identity":"u4","tracks":[{"type":"VIDEO","name":"screen","source":"CAMERA","muted":true}]}"#,
        );
        assert!(!paused.streaming());
        assert!(!participant(r#"{"identity":"u5"}"#).streaming());
    }

    #[test]
    fn snapshot_says_who_streams() {
        let mut store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap().as_str().to_owned();
        for id in ["ann", "bob"] {
            let device = Device {
                user_id: id,
                public_key: id.as_bytes(),
                name: id,
            };
            store.login(device, Some(&join)).unwrap().unwrap();
        }
        let rooms = HashMap::from([(
            "c0".to_owned(),
            vec![
                participant(r#"{"identity":"bob"}"#),
                participant(
                    r#"{"identity":"ann","tracks":[{"type":"VIDEO","name":"screen","source":"CAMERA"}]}"#,
                ),
            ],
        )]);
        let state = snapshot(&store, &rooms).unwrap();
        let people: Vec<_> = state.presence[0]
            .members
            .iter()
            .map(|m| (m.user_id.as_str(), m.streaming))
            .collect();
        assert_eq!(people, [("ann", true), ("bob", false)]);
    }

    #[test]
    fn snapshot_keeps_channel_order_and_only_this_camps_people() {
        let mut store = Store::open_in_memory("x").unwrap();
        let join = store.join_token().unwrap().as_str().to_owned();
        for (id, name) in [("bob", "Боб"), ("ann", "Аня")] {
            let device = Device {
                user_id: id,
                public_key: id.as_bytes(),
                name,
            };
            store.login(device, Some(&join)).unwrap().unwrap();
        }
        let rooms = HashMap::from([
            (
                "afk".to_owned(),
                vec![
                    participant(r#"{"identity":"bob"}"#),
                    participant(r#"{"identity":"ann"}"#),
                ],
            ),
            (
                "c0".to_owned(),
                vec![participant(r#"{"identity":"stranger"}"#)],
            ),
            (
                // A text channel never has a room; a stray one is ignored all the same.
                "c1".to_owned(),
                vec![participant(r#"{"identity":"ann"}"#)],
            ),
            (
                "other-camp-room".to_owned(),
                vec![participant(r#"{"identity":"bob"}"#)],
            ),
        ]);
        let state = snapshot(&store, &rooms).unwrap();
        assert_eq!(state.members.len(), 2);
        assert_eq!(
            state.presence.len(),
            1,
            "c0 had only a stranger; text channels and unknown rooms ignored"
        );
        assert_eq!(state.presence[0].channel_id, "afk");
        assert_eq!(state.name, "x", "the snapshot carries the camp itself");
        assert_eq!(state.channels.len(), 3);
        let ids: Vec<_> = state.presence[0]
            .members
            .iter()
            .map(|m| m.user_id.as_str())
            .collect();
        assert_eq!(ids, ["ann", "bob"], "sorted for a stable snapshot");
    }
}
