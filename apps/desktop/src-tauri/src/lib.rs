//! OurCamp desktop client — native side.
//!
//! The webview renders the UI; everything that touches audio lives here so it never
//! passes through the browser engine (docs/decisions/0002-native-media-engine.md).

mod audio;
mod camps;
mod commands;
mod levels;
mod participants;
mod voice;

use std::path::PathBuf;
use std::sync::Arc;

use audio::engine::Engine;
use camps::Camps;
use commands::{CampsState, EngineState};
use tauri::Manager;

/// Live camp state for the webview. Must match `CAMP_STATE_EVENT` in
/// `src/engine/tauri-camp-api.ts`.
const CAMP_STATE_EVENT: &str = "camp://state";

/// Starts the application. Called from `main.rs` (desktop) and mobile entry points.
///
/// # Panics
/// If Tauri fails to start (missing webview runtime, invalid config).
pub fn run() {
    // Before any room connects: LiveKit signalling goes only to pinned camps, over TLS,
    // always (ADR-0019, ADR-0020).
    oc_camp_client::transport::install(oc_camp_client::transport::PlainLoopback::Deny);
    let engine: EngineState = Engine::start();
    if let Err(reason) = &engine {
        eprintln!("[ourcamp] {reason}");
    }
    let for_levels = engine.clone().ok();

    tauri::Builder::default()
        .manage(engine)
        .setup(move |app| {
            let dir = data_dir(app.path().app_data_dir());
            if let Ok(dir) = &dir {
                eprintln!("[ourcamp] data: {}", dir.display());
                match lock_profile(dir) {
                    Ok(lock) => {
                        app.manage(lock);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // A second window would join voice as the same person and LiveKit
                        // would throw the first one out (`DuplicateIdentity`).
                        eprintln!(
                            "[ourcamp] OurCamp is already running with this profile; \n                             for a second person on this PC set OC_PROFILE=<name> (debug builds)"
                        );
                        std::process::exit(0);
                    }
                    // Cannot lock (exotic file system): run anyway rather than not at all.
                    Err(e) => eprintln!("[ourcamp] profile lock unavailable: {e}"),
                }
            }
            let camps: CampsState = dir
                .and_then(|dir| Camps::load(&dir).map_err(|e| e.to_string()))
                .map(Arc::new)
                .map_err(|e| format!("Папка данных приложения недоступна: {e}"));
            if let Err(reason) = &camps {
                eprintln!("[ourcamp] {reason}");
            }
            if let Ok(camps) = &camps {
                let app = app.handle().clone();
                camps.set_state_listener(Arc::new(move |live| {
                    if let Err(e) = tauri::Emitter::emit(&app, CAMP_STATE_EVENT, live) {
                        eprintln!("[ourcamp] camp state event: {e}");
                    }
                }));
            }
            app.manage(camps);
            if let Some(engine) = for_levels {
                participants::forward(app.handle().clone(), &engine);
                levels::spawn(app.handle().clone(), engine);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::voice_join_channel,
            commands::voice_leave_channel,
            commands::voice_set_volume,
            commands::voice_set_local_mute,
            commands::voice_set_mic_muted,
            commands::voice_set_deafened,
            commands::camp_list,
            commands::camp_preview,
            commands::camp_join,
            commands::camp_open,
            commands::camp_forget,
            commands::camp_invite,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start OurCamp");
}

/// The app's data directory. Debug builds honour `OC_PROFILE=<name>`: a separate
/// directory, so a second client on the same machine is a different device (its own key,
/// its own camps) — two people talking on one PC during development.
fn data_dir(base: tauri::Result<PathBuf>) -> Result<PathBuf, String> {
    let base = base.map_err(|e| e.to_string())?;
    let profile = std::env::var("OC_PROFILE")
        .ok()
        .filter(|p| cfg!(debug_assertions) && !p.trim().is_empty());
    Ok(match profile {
        Some(p) => base.join("profiles").join(p.trim()),
        None => base,
    })
}

/// Held for the life of the process: one running OurCamp per profile (data directory).
/// The OS releases the lock when the process ends, however it ends.
struct ProfileLock(#[allow(dead_code, reason = "held only to keep the lock")] std::fs::File);

fn lock_profile(dir: &std::path::Path) -> std::io::Result<ProfileLock> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(ProfileLock(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(std::io::ErrorKind::WouldBlock.into()),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::lock_profile;

    #[test]
    fn a_profile_can_be_locked_once_until_released() {
        let dir = std::env::temp_dir().join(format!("oc-lock-{}", std::process::id()));
        let first = lock_profile(&dir).expect("first lock");
        let second = lock_profile(&dir).err().expect("second must fail");
        assert_eq!(second.kind(), std::io::ErrorKind::WouldBlock);
        let other = lock_profile(&dir.join("profiles").join("artem"));
        assert!(other.is_ok(), "another profile is independent");
        drop(first);
        assert!(
            lock_profile(&dir).is_ok(),
            "released when the holder goes away"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
