//! OurCamp desktop client — native side.
//!
//! The webview renders the UI; everything that touches audio lives here so it never
//! passes through the browser engine (docs/decisions/0002-native-media-engine.md).

mod audio;
mod commands;
mod levels;
mod participants;
mod voice;

use audio::engine::Engine;
use commands::EngineState;

/// Starts the application. Called from `main.rs` (desktop) and mobile entry points.
///
/// # Panics
/// If Tauri fails to start (missing webview runtime, invalid config).
pub fn run() {
    let engine: EngineState = Engine::start();
    if let Err(reason) = &engine {
        eprintln!("[ourcamp] {reason}");
    }
    let for_levels = engine.clone().ok();

    tauri::Builder::default()
        .manage(engine)
        .setup(move |app| {
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
        ])
        .run(tauri::generate_context!())
        .expect("failed to start OurCamp");
}
