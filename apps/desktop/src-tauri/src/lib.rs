//! OurCamp desktop client — native side.
//!
//! The webview renders the UI; everything that touches audio lives here so it never
//! passes through the browser engine (docs/decisions/0002-native-media-engine.md).

mod audio;
mod camps;
mod commands;
mod levels;
mod participants;
mod stream;
mod voice;
mod windows;

use std::path::PathBuf;
use std::sync::Arc;

use audio::engine::Engine;
use camps::Camps;
use commands::{CampsState, EngineState};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

/// Live camp state for the webview. Must match `CAMP_STATE_EVENT` in
/// `src/engine/tauri-camp-api.ts`.
const CAMP_STATE_EVENT: &str = "camp://state";
/// What the camp asked of this member alone (ADR-0028). Must match
/// `CAMP_DIRECTIVE_EVENT` in `src/engine/tauri-camp-api.ts`.
const CAMP_DIRECTIVE_EVENT: &str = "camp://directive";
/// Chat updates for every window (ADR-0044). Must match `CAMP_CHAT_EVENT` in
/// `src/engine/tauri-camp-api.ts`.
const CAMP_CHAT_EVENT: &str = "camp://chat";
/// The system's sound devices changed (ADR-0066). Must match `SOUND_EVENT` in
/// `src/engine/tauri-sound-api.ts`.
const SOUND_EVENT: &str = "sound://changed";

/// The main window's close button hides it into the tray: voice and the camp stay on, like
/// in other voice apps; "Выйти" in the tray menu ends OurCamp (ADR-0031). The chat window
/// simply closes (ADR-0046). The picture goes before the window that holds it (`Viewer::close`).
fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        if window.label() == windows::MAIN {
            api.prevent_close();
            let _ = window.hide();
        }
        if window.label() == windows::STREAM {
            api.prevent_close();
            tauri::async_runtime::spawn(close_viewer(window.clone()));
        }
    }
    // The page's own goodbye does not run when its window goes.
    if matches!(event, WindowEvent::Destroyed) && window.label() == windows::SETTINGS {
        tauri::async_runtime::spawn(end_mic_check(window.clone()));
    }
}

/// Closes the microphone the settings' bar opened (ADR-0069).
async fn end_mic_check(window: tauri::Window) {
    let state = window.state::<commands::EngineState>();
    if let Ok(engine) = commands::engine(&state) {
        if let Err(e) = engine.mic_check(false).await {
            eprintln!("[oc-audio] {e}");
        }
    }
}

/// Closes the viewer, which destroys its window; without a viewer, just the window.
async fn close_viewer(window: tauri::Window) {
    let state = window.state::<commands::EngineState>();
    let closed = match commands::engine(&state) {
        Ok(engine) => engine.stop_watching().await,
        Err(_) => false,
    };
    if !closed {
        let _ = window.destroy();
    }
}

/// Sends what the camps say to every window: live state, your directives, chats.
fn forward_camp_events(app: &tauri::AppHandle, camps: &Camps) {
    let to_ui = app.clone();
    camps.set_state_listener(Arc::new(move |live| {
        if let Err(e) = tauri::Emitter::emit(&to_ui, CAMP_STATE_EVENT, live) {
            eprintln!("[ourcamp] camp state event: {e}");
        }
    }));
    let to_ui = app.clone();
    camps.set_directive_listener(Arc::new(move |directive| {
        if let Err(e) = tauri::Emitter::emit(&to_ui, CAMP_DIRECTIVE_EVENT, directive) {
            eprintln!("[ourcamp] camp directive event: {e}");
        }
    }));
    let to_ui = app.clone();
    camps.set_chat_listener(Arc::new(move |update| {
        if let Err(e) = tauri::Emitter::emit(&to_ui, CAMP_CHAT_EVENT, update) {
            eprintln!("[ourcamp] camp chat event: {e}");
        }
    }));
}

/// One running OurCamp per profile, or this one leaves.
fn hold_profile(app: &tauri::App, dir: &std::path::Path) {
    match lock_profile(dir) {
        Ok(lock) => {
            app.manage(lock);
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            // A second window would join voice as the same person and LiveKit
            // would throw the first one out (`DuplicateIdentity`).
            eprintln!(
                "[ourcamp] OurCamp is already running with this profile; \
                 for a second person on this PC set OC_PROFILE=<name> (debug builds)"
            );
            std::process::exit(0);
        }
        // Cannot lock (exotic file system): run anyway rather than not at all.
        Err(e) => eprintln!("[ourcamp] profile lock unavailable: {e}"),
    }
}

/// Tells every window when the system's sound devices change.
fn forward_sound(app: tauri::AppHandle, engine: &Engine) {
    engine.watch_devices(Arc::new(move |sound| {
        if let Err(e) = tauri::Emitter::emit(&app, SOUND_EVENT, sound) {
            eprintln!("[ourcamp] sound event: {e}");
        }
    }));
}

/// Everything the pages may call.
fn commands() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        commands::voice_join_channel,
        commands::voice_leave_channel,
        commands::voice_set_volume,
        commands::voice_set_local_mute,
        commands::voice_set_mic_muted,
        commands::voice_set_deafened,
        commands::settings_window_open,
        commands::sound_view,
        commands::sound_set_output,
        commands::sound_set_microphone,
        commands::sound_set_volume,
        commands::sound_set_mic_mode,
        commands::sound_set_threshold,
        commands::sound_set_talk_key,
        commands::sound_mic_check,
        commands::sound_mic_input,
        commands::profile_set_name,
        commands::stream_sources,
        commands::stream_qualities,
        commands::stream_sound_available,
        commands::stream_volume,
        commands::stream_start_window_open,
        commands::stream_start,
        commands::stream_stop,
        commands::stream_watch,
        commands::stream_view,
        commands::stream_place,
        commands::camp_list,
        commands::camp_preview,
        commands::camp_join,
        commands::camp_open,
        commands::camp_leave,
        commands::camp_invite,
        commands::camp_move_member,
        commands::camp_kick,
        commands::camp_set_silenced,
        commands::camp_avatar,
        commands::chat_history,
        commands::chat_send,
        commands::chat_photo,
        commands::chat_delete,
        commands::chat_mark_read,
        commands::chat_context,
        commands::chat_window_open,
        commands::profile_avatar,
        commands::profile_set_avatar,
    ]
}

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
    let for_exit = engine.clone().ok();

    let mut builder = tauri::Builder::default();
    // Registered first, as the plugin asks: launching OurCamp again while it sits in the
    // tray brings its window back instead of quietly exiting (ADR-0031). Debug profiles
    // (`OC_PROFILE`) are other people on the same PC and run side by side.
    if profile().is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app);
        }));
    }
    let app = builder
        .manage(engine)
        .setup(move |app| {
            let dir = data_dir(app.path().app_data_dir());
            if let Ok(dir) = &dir {
                eprintln!("[ourcamp] data: {}", dir.display());
                if let Some(engine) = &for_levels {
                    engine.load_sound(dir.clone());
                }
                hold_profile(app, dir);
            }
            let camps: CampsState = dir
                .and_then(|dir| Camps::load(&dir).map_err(|e| e.to_string()))
                .map(Arc::new)
                .map_err(|e| format!("Папка данных приложения недоступна: {e}"));
            if let Err(reason) = &camps {
                eprintln!("[ourcamp] {reason}");
            }
            if let Ok(camps) = &camps {
                forward_camp_events(app.handle(), camps);
            }
            app.manage(camps);
            if let Some(engine) = for_levels {
                participants::forward(app.handle().clone(), &engine);
                forward_sound(app.handle().clone(), &engine);
                levels::spawn(app.handle().clone(), engine);
            }
            tray(app)?;
            Ok(())
        })
        .on_window_event(on_window_event)
        .invoke_handler(commands())
        .build(tauri::generate_context!())
        .expect("failed to start OurCamp");
    app.run(move |_, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(engine) = &for_exit {
                leave_before_exit(engine);
            }
        }
    });
}

/// Shows the window again (from the tray or a second launch).
fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(windows::MAIN) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The tray icon: a click opens the window, the menu also quits.
fn tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выйти из OurCamp", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("OurCamp")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            // `RunEvent::Exit` leaves the channel politely before the process ends.
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// How long closing the app waits for a polite leave; a camp that does not answer must
/// not hold the window open.
const LEAVE_ON_EXIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Leaves the campfire before the process ends. Without it LiveKit only notices the
/// person is gone when it times the connection out, and until then everyone — the
/// server window included — still sees them sitting there.
fn leave_before_exit(engine: &Engine) {
    tauri::async_runtime::block_on(async {
        match tokio::time::timeout(LEAVE_ON_EXIT, engine.leave_channel()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("[ourcamp] leave on exit: {e}"),
            Err(_) => eprintln!("[ourcamp] leave on exit: no answer in {LEAVE_ON_EXIT:?}"),
        }
    });
}

/// The app's data directory. Debug builds honour `OC_PROFILE=<name>`: a separate
/// directory, so a second client on the same machine is a different device (its own key,
/// its own camps) — two people talking on one PC during development.
fn data_dir(base: tauri::Result<PathBuf>) -> Result<PathBuf, String> {
    let base = base.map_err(|e| e.to_string())?;
    Ok(match profile() {
        Some(p) => base.join("profiles").join(p),
        None => base,
    })
}

/// `OC_PROFILE` in debug builds: a second person on this PC.
fn profile() -> Option<String> {
    std::env::var("OC_PROFILE")
        .ok()
        .filter(|p| cfg!(debug_assertions) && !p.trim().is_empty())
        .map(|p| p.trim().to_owned())
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
