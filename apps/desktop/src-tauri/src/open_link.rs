//! Opens a link from a chat in the system's browser (ADR-0077). Only addresses of the web get
//! this far: a chat must not be able to start programs or open files on this computer.

/// Hands `link` to the system's browser.
///
/// # Errors
/// A sentence for the window: not an address of the web, or no browser to take it.
pub fn open(link: &str) -> Result<(), String> {
    let address =
        oc_link_preview::web_address(link).ok_or_else(|| "Эту ссылку открыть нельзя".to_owned())?;
    hand_over(&address).map_err(|e| {
        eprintln!("[ourcamp] open link: {e}");
        "Не получилось открыть браузер".to_owned()
    })
}

#[cfg(windows)]
fn hand_over(address: &str) -> Result<(), String> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide: Vec<u16> = address.encode_utf16().chain(Some(0)).collect();
    // SAFETY: both strings are NUL-terminated and outlive the call; it keeps neither.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Not a real handle: above 32 means success, the rest are error codes.
    if result.0 as usize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute returned {}", result.0 as usize))
    }
}

#[cfg(not(windows))]
fn hand_over(address: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(address)
        .spawn()
        .map(drop)
        .map_err(|e| e.to_string())
}
