//! Pushes member levels to the UI for the speaking ring.
//!
//! Polling the mixer's atomics at 20 Hz is cheaper than pushing from the audio thread
//! (which must not touch IPC) and fast enough that the ring feels instant.

use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::audio::engine::Engine;

/// Must match `LEVELS_EVENT` in `src/engine/tauri-engine.ts`.
pub const LEVELS_EVENT: &str = "voice://levels";
const INTERVAL: Duration = Duration::from_millis(50);

#[derive(Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct LevelDto {
    user_id: String,
    level_db: f32,
    speaking: bool,
}

/// Starts the emitter thread for the lifetime of the app.
pub fn spawn(app: AppHandle, engine: Engine) {
    let result = thread::Builder::new()
        .name("oc-levels".into())
        .spawn(move || {
            let mut last_was_empty = true;
            loop {
                thread::sleep(INTERVAL);
                let levels: Vec<LevelDto> = engine
                    .levels()
                    .into_iter()
                    .map(|l| LevelDto {
                        user_id: l.user_id,
                        // Rounded: the UI only needs whole dB and it keeps payloads small.
                        level_db: l.level_db.round(),
                        speaking: l.speaking,
                    })
                    .collect();
                // While nobody is in the channel, send one empty update and then go quiet.
                if levels.is_empty() && last_was_empty {
                    continue;
                }
                last_was_empty = levels.is_empty();
                if app.emit(LEVELS_EVENT, &levels).is_err() {
                    break; // app is shutting down
                }
            }
        });
    if let Err(e) = result {
        eprintln!("[oc-levels] failed to start: {e}");
    }
}
