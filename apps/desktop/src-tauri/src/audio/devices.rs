//! Which sound devices there are and which to use, and what the user chose (ADR-0066).
//!
//! `cpal` gives devices no id of their own, so a device is known by its name. Windows keeps
//! these names apart ("2- USB Audio"), and they survive unplugging and restarts.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use cpal::traits::{DeviceTrait, HostTrait};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

const SOUND_FILE: &str = "sound.json";

/// What the system has right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub outputs: Vec<String>,
    pub default_output: Option<String>,
    pub microphones: Vec<String>,
    pub default_microphone: Option<String>,
}

/// Asks the system. Off the UI thread: it takes a moment for each device.
#[must_use]
pub fn list() -> Listing {
    let host = cpal::default_host();
    let names = |devices: Option<Vec<cpal::Device>>| -> Vec<String> {
        devices
            .unwrap_or_default()
            .iter()
            .filter_map(|d| d.name().ok())
            .collect()
    };
    Listing {
        outputs: names(host.output_devices().ok().map(Iterator::collect)),
        default_output: host.default_output_device().and_then(|d| d.name().ok()),
        microphones: names(host.input_devices().ok().map(Iterator::collect)),
        default_microphone: host.default_input_device().and_then(|d| d.name().ok()),
    }
}

/// What changes when the system's devices do. Unlike [`list`], which tries every device's
/// formats to tell outputs from microphones (200 ms for four devices), this only reads names,
/// so the watcher can do it all the time.
#[must_use]
pub fn glance() -> Vec<Option<String>> {
    let host = cpal::default_host();
    let name = |device: Option<cpal::Device>| device.and_then(|d| d.name().ok());
    let mut seen = vec![
        name(host.default_output_device()),
        name(host.default_input_device()),
    ];
    if let Ok(devices) = host.devices() {
        seen.extend(devices.map(|d| d.name().ok()));
    }
    seen
}

/// The device to use: the chosen one while it is connected, otherwise the system's.
#[must_use]
pub fn pick<'a>(
    chosen: Option<&'a str>,
    connected: &[String],
    system: Option<&'a str>,
) -> Option<&'a str> {
    chosen
        .filter(|name| connected.iter().any(|c| c == name))
        .or(system)
}

/// What the user chose; `None` follows the system. Kept on this device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SoundPrefs {
    pub microphone: Option<String>,
    pub output: Option<String>,
    /// Overall volume, percent.
    pub volume: f32,
}

impl Default for SoundPrefs {
    fn default() -> Self {
        Self {
            microphone: None,
            output: None,
            volume: 100.0,
        }
    }
}

impl SoundPrefs {
    /// Reads the choice; a missing or damaged file means none was made. Losing it costs a
    /// click, not worth refusing to start.
    #[must_use]
    pub fn load(dir: &Path) -> Self {
        let file = dir.join(SOUND_FILE);
        match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("[oc-audio] {} is damaged: {e}", file.display());
                Self::default()
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!("[oc-audio] {}: {e}", file.display());
                Self::default()
            }
        }
    }

    /// Writes the choice. Through a temporary file: a crash must not leave half of it.
    ///
    /// # Errors
    /// If the file cannot be written.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temporary = dir.join(format!("{SOUND_FILE}.tmp"));
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, dir.join(SOUND_FILE))
    }
}

/// Raised from a device's error callback (a headset pulled out, a driver reset): the watcher
/// reopens the device right away instead of at its next look.
#[derive(Debug, Default)]
pub struct Alarm {
    output: AtomicBool,
    microphone: AtomicBool,
    wake: Notify,
}

impl Alarm {
    pub fn output_failed(&self) {
        self.output.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }

    pub fn microphone_failed(&self) {
        self.microphone.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }

    /// Tries the output again at the next look, without waking anyone.
    pub fn retry_output(&self) {
        self.output.store(true, Ordering::Relaxed);
    }

    /// Tries the microphone again at the next look, without waking anyone.
    pub fn retry_microphone(&self) {
        self.microphone.store(true, Ordering::Relaxed);
    }

    /// Whether the output failed since the last call.
    pub fn take_output(&self) -> bool {
        self.output.swap(false, Ordering::Relaxed)
    }

    /// Whether the microphone failed since the last call.
    pub fn take_microphone(&self) -> bool {
        self.microphone.swap(false, Ordering::Relaxed)
    }

    /// Waits for the next failure.
    pub async fn raised(&self) {
        self.wake.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oc-sound-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_chosen_device_is_used_while_connected() {
        let connected = names(&["Наушники", "Динамики"]);
        assert_eq!(
            pick(Some("Наушники"), &connected, Some("Динамики")),
            Some("Наушники")
        );
    }

    #[test]
    fn without_the_chosen_device_the_systems_is_used() {
        let connected = names(&["Динамики"]);
        assert_eq!(
            pick(Some("Наушники"), &connected, Some("Динамики")),
            Some("Динамики")
        );
        assert_eq!(pick(None, &connected, Some("Динамики")), Some("Динамики"));
    }

    #[test]
    fn with_no_devices_there_is_nothing_to_use() {
        assert_eq!(pick(Some("Наушники"), &[], None), None);
    }

    #[test]
    fn the_choice_survives_a_restart() {
        let dir = temp_dir("saved");
        let prefs = SoundPrefs {
            microphone: Some("Гарнитура".into()),
            output: None,
            volume: 40.0,
        };
        prefs.save(&dir).unwrap();
        assert_eq!(SoundPrefs::load(&dir), prefs);
    }

    #[test]
    fn no_file_or_a_damaged_one_means_no_choice() {
        let dir = temp_dir("damaged");
        assert_eq!(SoundPrefs::load(&dir), SoundPrefs::default());
        fs::write(dir.join(SOUND_FILE), b"{not json").unwrap();
        assert_eq!(SoundPrefs::load(&dir), SoundPrefs::default());
    }

    #[test]
    fn a_failure_is_told_once() {
        let alarm = Alarm::default();
        assert!(!alarm.take_output());
        alarm.output_failed();
        assert!(alarm.take_output());
        assert!(!alarm.take_output());
        assert!(!alarm.take_microphone(), "the other device is fine");
    }
}
