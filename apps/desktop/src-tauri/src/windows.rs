//! Windows besides the main one (ADR-0046): the chat opens in its own, next to the main
//! window, like the mockup's screen 9. It is a page of the same frontend (`chat.html`)
//! and talks to the same Rust state; closing it closes it, unlike the main window.

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

/// The chat window's label. Must match `capabilities/default.json`.
pub const CHAT: &str = "chat";
/// The main window's label (`tauri.conf.json`).
pub const MAIN: &str = "main";

/// Logical size of a new chat window, as in the mockup.
const CHAT_SIZE: (f64, f64) = (420.0, 660.0);
/// Gap between the main window and the chat beside it, in logical pixels.
const GAP: f64 = 12.0;

/// Shows the chat window, opening it beside the main window the first time.
///
/// # Errors
/// If the webview cannot be created.
pub fn open_chat(app: &AppHandle) -> tauri::Result<()> {
    if let Some(chat) = app.get_webview_window(CHAT) {
        chat.unminimize()?;
        chat.show()?;
        return chat.set_focus();
    }
    let chat = WebviewWindowBuilder::new(app, CHAT, WebviewUrl::App("chat.html".into()))
        .title("Чат — OurCamp")
        .inner_size(CHAT_SIZE.0, CHAT_SIZE.1)
        .min_inner_size(340.0, 420.0)
        .decorations(false)
        .background_color(tauri::window::Color(0x10, 0x10, 0x10, 0xff))
        .visible(false)
        .build()?;
    place_beside_main(app, &chat);
    chat.show()?;
    chat.set_focus()
}

/// To the right of the main window when the screen has room, else where the system puts
/// it. Placement is a nicety: any failure leaves the default.
fn place_beside_main(app: &AppHandle, chat: &tauri::WebviewWindow) {
    let Some(main) = app.get_webview_window(MAIN) else {
        return;
    };
    let (Ok(position), Ok(size), Ok(scale)) = (
        main.outer_position(),
        main.outer_size(),
        main.scale_factor(),
    ) else {
        return;
    };
    let x = f64::from(position.x) + f64::from(size.width) + GAP * scale;
    let fits = main
        .current_monitor()
        .ok()
        .flatten()
        .is_some_and(|monitor| {
            let right = f64::from(monitor.position().x) + f64::from(monitor.size().width);
            x + CHAT_SIZE.0 * scale <= right
        });
    if fits {
        // Pixels are whole; the rounding is the intent.
        #[allow(clippy::cast_possible_truncation)]
        let _ = chat.set_position(PhysicalPosition::new(x.round() as i32, position.y));
    }
}
