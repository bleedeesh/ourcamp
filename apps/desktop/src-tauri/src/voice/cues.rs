//! Which of a room's events deserve a sound (ADR-0075).
//!
//! A lost connection must stay silent, and the SDK makes that harder than it looks: on a full
//! reconnect it first reports everyone as gone, only then says it is reconnecting, and later
//! reports them all as new. So a departure waits a moment before it is heard, and whoever was
//! here before the reconnect is not news after it.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use oc_audio::cue::Cue;

/// How long a departure waits: long enough for "reconnecting" to catch up with it.
const LEFT_DELAY: Duration = Duration::from_millis(200);

pub struct RoomCues {
    /// Who we consider present, with those whose departure is still waiting.
    known: HashSet<String>,
    /// The departures waiting, and when the sound is due.
    leaving: Vec<String>,
    due: Option<Instant>,
    reconnecting: bool,
}

impl RoomCues {
    /// `present`: who was in the room as we came. They are not news.
    pub fn new(present: impl IntoIterator<Item = String>) -> Self {
        Self {
            known: present.into_iter().collect(),
            leaving: Vec::new(),
            due: None,
            reconnecting: false,
        }
    }

    /// Someone appeared.
    pub fn joined(&mut self, user_id: String) -> Option<Cue> {
        let new = self.known.insert(user_id);
        (new && !self.reconnecting).then_some(Cue::Joined)
    }

    /// Someone is gone, as far as the room says. Heard at [`Self::due`], unless it turns out
    /// to be a reconnect.
    pub fn left(&mut self, user_id: String, now: Instant) {
        if self.reconnecting || !self.known.contains(&user_id) {
            return;
        }
        self.leaving.push(user_id);
        self.due.get_or_insert(now + LEFT_DELAY);
    }

    /// When [`Self::departures`] has something to say.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// The time has come: everyone who left together makes one sound.
    pub fn departures(&mut self) -> Option<Cue> {
        self.due = None;
        let leaving = std::mem::take(&mut self.leaving);
        for user_id in &leaving {
            self.known.remove(user_id);
        }
        (!leaving.is_empty()).then_some(Cue::Left)
    }

    /// Someone began a stream.
    pub fn stream_started(&self) -> Option<Cue> {
        (!self.reconnecting).then_some(Cue::StreamStarted)
    }

    /// The connection is being restored: what was waiting was not a departure.
    pub fn reconnecting(&mut self) {
        self.reconnecting = true;
        self.leaving.clear();
        self.due = None;
    }

    /// It is back. `present`: who the room has now; whoever joined meanwhile makes no sound.
    pub fn reconnected(&mut self, present: impl IntoIterator<Item = String>) {
        self.reconnecting = false;
        self.known.extend(present);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cues(present: &[&str]) -> RoomCues {
        RoomCues::new(present.iter().map(ToString::to_string))
    }

    #[test]
    fn someone_new_is_heard_once_and_those_already_here_are_not() {
        let mut cues = cues(&["anya"]);
        assert_eq!(cues.joined("anya".into()), None);
        assert_eq!(cues.joined("kostya".into()), Some(Cue::Joined));
        assert_eq!(cues.joined("kostya".into()), None);
    }

    #[test]
    fn a_departure_is_heard_after_a_moment() {
        let mut cues = cues(&["anya"]);
        let now = Instant::now();
        assert_eq!(cues.due(), None);
        cues.left("anya".into(), now);
        assert_eq!(cues.due(), Some(now + LEFT_DELAY));
        assert_eq!(cues.departures(), Some(Cue::Left));
        assert_eq!(cues.due(), None);
        assert_eq!(cues.departures(), None);
    }

    #[test]
    fn who_came_back_after_leaving_is_heard_again() {
        let mut cues = cues(&["anya"]);
        cues.left("anya".into(), Instant::now());
        cues.departures();
        assert_eq!(cues.joined("anya".into()), Some(Cue::Joined));
    }

    #[test]
    fn several_leaving_together_make_one_sound() {
        let mut cues = cues(&["anya", "kostya"]);
        let now = Instant::now();
        cues.left("anya".into(), now);
        cues.left("kostya".into(), now + Duration::from_millis(50));
        assert_eq!(cues.due(), Some(now + LEFT_DELAY));
        assert_eq!(cues.departures(), Some(Cue::Left));
        assert_eq!(cues.departures(), None);
    }

    #[test]
    fn someone_we_never_saw_leaving_is_not_heard() {
        let mut cues = cues(&[]);
        cues.left("anya".into(), Instant::now());
        assert_eq!(cues.due(), None);
    }

    /// The order the SDK really has: everyone gone, then "reconnecting", then all of them new.
    #[test]
    fn a_full_reconnect_is_silent() {
        let mut cues = cues(&["anya", "kostya"]);
        let now = Instant::now();
        cues.left("anya".into(), now);
        cues.left("kostya".into(), now);
        cues.reconnecting();
        assert_eq!(cues.due(), None);
        assert_eq!(cues.departures(), None);
        assert_eq!(cues.joined("anya".into()), None);
        cues.reconnected(["anya".to_owned()]);
        // Some come after it is over.
        assert_eq!(cues.joined("kostya".into()), None);
    }

    #[test]
    fn nothing_is_heard_while_reconnecting_and_all_is_as_usual_after() {
        let mut cues = cues(&["anya"]);
        cues.reconnecting();
        assert_eq!(cues.joined("kostya".into()), None);
        assert_eq!(cues.stream_started(), None);
        cues.left("anya".into(), Instant::now());
        assert_eq!(cues.due(), None);
        cues.reconnected(["anya".to_owned(), "kostya".to_owned()]);
        assert_eq!(cues.joined("kostya".into()), None);
        assert_eq!(cues.joined("lena".into()), Some(Cue::Joined));
        assert_eq!(cues.stream_started(), Some(Cue::StreamStarted));
    }
}
