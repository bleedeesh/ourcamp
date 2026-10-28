//! # OurCamp Server window (ADR-0023)
//!
//! The camp of `oc-server` runs in this process; the window shows its [`ServerStatus`]:
//! commands for the first draw, `server://status` with a full snapshot on every change.
//!
//! ```text
//! oc_server::start ─► Running ── camp_state() ─┐
//!                        └── livekit_pid_changes() ─► snapshot ─► server://status ─► UI
//! ```

mod manage;
mod status;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use oc_server::{find_livekit, lan_address, Config, LiveKitMode, ManagedLiveKit, Running};
use status::{snapshot, Base, ServerStatus};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tokio::sync::watch;

/// Event with the whole [`ServerStatus`].
const STATUS_EVENT: &str = "server://status";
/// Name of a camp created by the window; renaming comes later (ADR-0023).
const NEW_CAMP_NAME: &str = "Мой сервер";

/// The camp as the window holds it.
struct Host {
    data_dir: PathBuf,
    status: Mutex<ServerStatus>,
    running: tokio::sync::Mutex<Option<Running>>,
}

impl Host {
    fn status(&self) -> ServerStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn publish(&self, app: &tauri::AppHandle, status: ServerStatus) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status.clone();
        let _ = app.emit(STATUS_EVENT, status);
    }

    fn config(&self) -> Result<Config, String> {
        let binary = find_livekit().ok_or(
            "livekit-server не найден: он должен лежать рядом с OurCamp Server \
             (в разработке — `pnpm livekit:fetch`)",
        )?;
        Ok(Config {
            data_dir: self.data_dir.clone(),
            bind: SocketAddr::from((Ipv4Addr::UNSPECIFIED, oc_invite::DEFAULT_PORT)),
            public_host: lan_address(),
            public_port: None,
            camp_name: NEW_CAMP_NAME.into(),
            livekit: LiveKitMode::Managed(ManagedLiveKit::new(binary)),
        })
    }

    /// Starts the camp (again, after a failure) and follows its state.
    async fn start(self: Arc<Self>, app: tauri::AppHandle) {
        let mut running = self.running.lock().await;
        if running.is_some() {
            return;
        }
        self.publish(&app, ServerStatus::starting());
        let started = match self.config() {
            Ok(config) => oc_server::start(config).await.map_err(|e| e.to_string()),
            Err(e) => Err(e),
        };
        match started {
            Ok(camp) => {
                let base = Base::of(&camp);
                let state = camp.camp_state();
                let pid = camp.livekit_pid_changes();
                self.publish(&app, snapshot(&base, &state.borrow(), camp.livekit_pid()));
                tauri::async_runtime::spawn(Arc::clone(&self).follow(app, base, state, pid));
                *running = Some(camp);
            }
            Err(message) => {
                eprintln!("[ourcamp-server] {message}");
                self.publish(&app, ServerStatus::failed(message));
            }
        }
    }

    /// Republishes on every change of the camp or of LiveKit, until the camp stops
    /// (its senders drop).
    async fn follow(
        self: Arc<Self>,
        app: tauri::AppHandle,
        base: Base,
        mut state: watch::Receiver<Arc<oc_proto::CampState>>,
        mut pid: Option<watch::Receiver<Option<u32>>>,
    ) {
        loop {
            let pid_changed = async {
                match pid.as_mut() {
                    Some(pid) => pid.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            let alive = tokio::select! {
                changed = state.changed() => changed.is_ok(),
                alive = pid_changed => alive,
            };
            if !alive {
                return;
            }
            let livekit_pid = pid.as_ref().and_then(|p| *p.borrow());
            let status = snapshot(&base, &state.borrow_and_update(), livekit_pid);
            self.publish(&app, status);
        }
    }

    /// The running camp's owner handle; `None` while it is not running.
    async fn manager(&self) -> Option<oc_server::Manager> {
        self.running.lock().await.as_ref().map(Running::manager)
    }

    async fn stop(&self) {
        if let Some(camp) = self.running.lock().await.take() {
            camp.stop().await;
        }
    }
}

#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects `State` by value"
)]
fn server_status(host: tauri::State<'_, Arc<Host>>) -> ServerStatus {
    host.status()
}

#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects `State` by value"
)]
fn server_retry(app: tauri::AppHandle, host: tauri::State<'_, Arc<Host>>) {
    tauri::async_runtime::spawn(Arc::clone(&host).start(app));
}

/// Held for the whole run: one window per data directory, as in the client.
struct DirLock(#[allow(dead_code, reason = "held only to keep the lock")] std::fs::File);

fn lock_dir(dir: &Path) -> std::io::Result<DirLock> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(DirLock(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(std::io::ErrorKind::WouldBlock.into()),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Shows the window again (from the tray).
fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Puts the friends link on the clipboard (from the tray, the window may be hidden).
fn copy_join_link(app: &tauri::AppHandle) {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    let link = app.state::<Arc<Host>>().status().join_link;
    if let Some(link) = link {
        if let Err(e) = app.clipboard().write_text(link) {
            eprintln!("[ourcamp-server] clipboard: {e}");
        }
    }
}

/// The tray icon: a click opens the window, the menu also stops the camp.
fn tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Открыть", true, None::<&str>)?;
    let copy = MenuItem::with_id(
        app,
        "copy",
        "Скопировать ссылку для друзей",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Остановить сервер и выйти", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &copy, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("OurCamp Server")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "copy" => copy_join_link(app),
            // `RunEvent::Exit` below stops the camp and LiveKit before the process ends.
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

/// Runs the window until it is closed.
///
/// # Panics
/// If Tauri cannot start (no webview runtime), with its own message.
pub fn run() {
    let app = tauri::Builder::default()
        // Registered first, as the plugin asks: a second launch only brings this
        // window back (it may be hidden in the tray) and exits.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app);
        }))
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let lock = match lock_dir(&data_dir) {
                Ok(lock) => lock,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    eprintln!("[ourcamp-server] this camp is served by another process");
                    std::process::exit(0);
                }
                Err(e) => return Err(e.into()),
            };
            app.manage(lock);
            eprintln!("[ourcamp-server] data: {}", data_dir.display());
            let host = Arc::new(Host {
                data_dir,
                status: Mutex::new(ServerStatus::starting()),
                running: tokio::sync::Mutex::new(None),
            });
            tauri::async_runtime::spawn(Arc::clone(&host).start(app.handle().clone()));
            app.manage(host);
            tray(app)?;
            Ok(())
        })
        // The close button hides the window: the camp keeps running for friends;
        // it stops from the tray menu (ADR-0023).
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            server_status,
            server_retry,
            manage::server_rename_camp,
            manage::server_set_role,
            manage::server_create_channel,
            manage::server_rename_channel,
            manage::server_delete_channel,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri could not start");
    app.run(|app, event| {
        // The camp (and its LiveKit) stop before the process ends, in order.
        if let RunEvent::Exit = event {
            let host = Arc::clone(app.state::<Arc<Host>>().inner());
            tauri::async_runtime::block_on(async move { host.stop().await });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::lock_dir;

    #[test]
    fn a_data_directory_is_served_by_one_window() {
        let dir = std::env::temp_dir().join(format!("oc-server-lock-{}", std::process::id()));
        let first = lock_dir(&dir).expect("first lock");
        let second = lock_dir(&dir).err().expect("second must fail");
        assert_eq!(second.kind(), std::io::ErrorKind::WouldBlock);
        drop(first);
        lock_dir(&dir).expect("free again");
        let _ = std::fs::remove_dir_all(dir);
    }
}
