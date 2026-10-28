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
            let camps: CampsState = data_dir(app.path().app_data_dir())
                .and_then(|dir| {
                    eprintln!("[ourcamp] data: {}", dir.display());
                    Camps::load(&dir).map_err(|e| e.to_string())
                })
                .map(Arc::new)
                .map_err(|e| format!("Папка данных приложения недоступна: {e}"));
            if let Err(reason) = &camps {
                eprintln!("[ourcamp] {reason}");
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
