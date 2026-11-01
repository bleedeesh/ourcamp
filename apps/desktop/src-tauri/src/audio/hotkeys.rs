//! Keys that turn the microphone and the sound on and off, whichever window has the focus
//! (ADR-0071). One thread reads only the assigned keys: nothing else about the keyboard, and
//! the keys still reach the other apps.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::devices::talk_key_down;

/// How often the assigned keys are read: a press is at least a few times this long.
const LOOK_EVERY: Duration = Duration::from_millis(15);
/// The same with nothing assigned, only to notice an assignment.
const IDLE_EVERY: Duration = Duration::from_millis(250);

/// Windows virtual-key codes of the modifiers; either side of each.
const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;

const SHIFT: u32 = 1 << 16;
const CTRL: u32 = 1 << 17;
const ALT: u32 = 1 << 18;

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// Turns the microphone off or on.
    Mic,
    /// Turns the sound off or on.
    Sound,
}

/// A key with its modifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hotkey {
    /// Windows virtual-key code.
    pub code: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// What the settings call it.
    pub label: String,
}

impl Hotkey {
    /// The key in one number, for the thread reading it.
    fn packed(&self) -> u32 {
        u32::from(self.code)
            | if self.ctrl { CTRL } else { 0 }
            | if self.shift { SHIFT } else { 0 }
            | if self.alt { ALT } else { 0 }
    }
}

/// Which keys are assigned; 0 is none. The settings write, the thread reads, without locks.
#[derive(Debug, Default)]
pub struct HotkeyControl {
    mic: AtomicU32,
    sound: AtomicU32,
}

impl HotkeyControl {
    pub fn set(&self, action: Action, key: Option<&Hotkey>) {
        let slot = match action {
            Action::Mic => &self.mic,
            Action::Sound => &self.sound,
        };
        slot.store(key.map_or(0, Hotkey::packed), Ordering::Relaxed);
    }
}

/// Tells a press of one key from the key being held.
#[derive(Debug, Default)]
struct Presses {
    key: u32,
    was_down: bool,
}

impl Presses {
    /// Whether the assigned key went down just now, with exactly its modifiers. The key's own
    /// fall and rise decide: holding it and then pressing Ctrl is not a press of Ctrl+key, and
    /// holding it gives one press, not many.
    fn pressed(&mut self, key: u32, is_down: impl Fn(u16) -> bool) -> bool {
        if key != self.key {
            // A new assignment: the key is still down from choosing it.
            self.key = key;
            self.was_down = true;
        }
        if key == 0 {
            return false;
        }
        // The low 16 bits are the code, by how `packed` builds it.
        #[allow(clippy::cast_possible_truncation)]
        let down = is_down(key as u16);
        let press = down && !self.was_down;
        self.was_down = down;
        press
            && is_down(VK_CONTROL) == (key & CTRL != 0)
            && is_down(VK_SHIFT) == (key & SHIFT != 0)
            && is_down(VK_MENU) == (key & ALT != 0)
    }
}

/// Watches the keys for as long as the app lives.
pub fn watch(control: Arc<HotkeyControl>, on_press: impl Fn(Action) + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("oc-hotkeys".into())
        .spawn(move || {
            let (mut mic, mut sound) = (Presses::default(), Presses::default());
            loop {
                let (mic_key, sound_key) = (
                    control.mic.load(Ordering::Relaxed),
                    control.sound.load(Ordering::Relaxed),
                );
                if mic.pressed(mic_key, talk_key_down) {
                    on_press(Action::Mic);
                }
                if sound.pressed(sound_key, talk_key_down) {
                    on_press(Action::Sound);
                }
                thread::sleep(if mic_key == 0 && sound_key == 0 {
                    IDLE_EVERY
                } else {
                    LOOK_EVERY
                });
            }
        });
    if let Err(e) = spawned {
        eprintln!("[oc-audio] keys: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: u16 = 0x4D;

    fn key(ctrl: bool, shift: bool, alt: bool) -> u32 {
        Hotkey {
            code: M,
            ctrl,
            shift,
            alt,
            label: String::new(),
        }
        .packed()
    }

    /// Presses that have seen the key up once, as after assigning it.
    fn ready(key: u32) -> Presses {
        let mut presses = Presses::default();
        presses.pressed(key, |_| false);
        presses
    }

    #[test]
    fn a_press_counts_once_however_long_it_is_held() {
        let plain = key(false, false, false);
        let mut presses = ready(plain);
        assert!(!presses.pressed(plain, |_| false));
        assert!(presses.pressed(plain, |code| code == M));
        for _ in 0..100 {
            assert!(!presses.pressed(plain, |code| code == M));
        }
        assert!(!presses.pressed(plain, |_| false));
        assert!(presses.pressed(plain, |code| code == M), "the next press");
    }

    #[test]
    fn the_modifiers_must_be_exactly_those_assigned() {
        let ctrl_m = key(true, false, false);
        let held = |codes: &'static [u16]| move |code: u16| codes.contains(&code);

        assert!(ready(ctrl_m).pressed(ctrl_m, held(&[M, VK_CONTROL])));
        assert!(!ready(ctrl_m).pressed(ctrl_m, held(&[M])), "no Ctrl");
        assert!(
            !ready(ctrl_m).pressed(ctrl_m, held(&[M, VK_CONTROL, VK_SHIFT])),
            "Shift as well is another key"
        );
        let plain = key(false, false, false);
        assert!(
            !ready(plain).pressed(plain, held(&[M, VK_MENU])),
            "Alt+M is not M"
        );
    }

    #[test]
    fn a_modifier_pressed_after_the_key_is_not_a_press() {
        let ctrl_m = key(true, false, false);
        let mut presses = ready(ctrl_m);
        assert!(!presses.pressed(ctrl_m, |code| code == M));
        assert!(!presses.pressed(ctrl_m, |code| code == M || code == VK_CONTROL));
    }

    #[test]
    fn a_key_still_down_from_being_assigned_is_not_a_press() {
        let plain = key(false, false, false);
        let mut presses = Presses::default();
        assert!(!presses.pressed(plain, |code| code == M));
        assert!(!presses.pressed(plain, |_| false));
        assert!(presses.pressed(plain, |code| code == M));
    }

    #[test]
    fn nothing_assigned_reads_nothing() {
        let mut presses = Presses::default();
        assert!(!presses.pressed(0, |_| panic!("no key to read")));
    }

    #[test]
    fn the_control_keeps_each_action_apart() {
        let control = HotkeyControl::default();
        let hotkey = Hotkey {
            code: M,
            ctrl: true,
            shift: false,
            alt: false,
            label: "Ctrl+M".into(),
        };
        control.set(Action::Sound, Some(&hotkey));
        assert_eq!(control.mic.load(Ordering::Relaxed), 0);
        assert_eq!(control.sound.load(Ordering::Relaxed), hotkey.packed());
        control.set(Action::Sound, None);
        assert_eq!(control.sound.load(Ordering::Relaxed), 0);
    }
}
