//! The place of the picture inside the viewer's window (ADR-0053): a child window over
//! the webview. The page around it stays an ordinary page; only this rectangle is ours.

use std::sync::OnceLock;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, ScreenToClient, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetClientRect, GetParent, GetWindowLongPtrW, IsZoomed,
    RegisterClassExW, SetWindowLongPtrW, SetWindowPos, ShowWindowAsync, CREATESTRUCTW, CS_DBLCLKS,
    GWLP_USERDATA, HTTRANSPARENT, HWND_TOP, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA,
    WINDOW_EX_STYLE, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_NCCREATE, WM_NCDESTROY,
    WM_NCHITTEST, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
};

/// How far from the window's edges the frame belongs to resizing, in pixels at 100 %. As in
/// Tauri's own handles for a window without a frame, which lie under this one.
const RESIZE_BORDER: i32 = 5;

/// What was done to the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    DoubleClick,
    Escape,
}

/// Handles to a child window, usable from any thread: everything here is asynchronous.
#[derive(Debug, Clone, Copy)]
pub struct VideoSurface(isize);

impl VideoSurface {
    /// Creates it hidden, over everything in `parent`. On the thread that runs `parent`'s
    /// messages: a window belongs to the thread that made it.
    ///
    /// # Errors
    /// If the system refuses.
    pub fn create(
        parent: isize,
        on_input: impl Fn(Input) + Send + 'static,
    ) -> windows::core::Result<Self> {
        register_class()?;
        // Handed to the window: `window_proc` takes it back at `WM_NCDESTROY`.
        let handler: Handler = Box::new(on_input);
        let handler = Box::into_raw(Box::new(handler));
        // SAFETY: the parent handle comes from a live window; the handler pointer is valid
        // until `WM_NCDESTROY`, and is only used on this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                PCWSTR::null(),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                Some(HWND(parent as *mut _)),
                None,
                Some(instance()?.into()),
                Some(handler.cast()),
            )
        };
        match hwnd {
            Ok(hwnd) => Ok(Self(hwnd.0 as isize)),
            Err(e) => {
                // SAFETY: creation failed before `WM_NCCREATE` could take it, or it took it
                // and destroyed nothing yet; a failed `WM_NCCREATE` is not used here.
                drop(unsafe { Box::from_raw(handler) });
                Err(e)
            }
        }
    }

    /// Handle for `wgpu`.
    pub fn raw(self) -> isize {
        self.0
    }

    fn hwnd(self) -> HWND {
        HWND(self.0 as *mut _)
    }

    /// Moves it, in the parent's pixels.
    pub fn place(self, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: a call on a window that may be gone only fails. `SWP_ASYNCWINDOWPOS` does
        // not wait for the window's thread, so this is safe from any thread.
        let moved = unsafe {
            SetWindowPos(
                self.hwnd(),
                Some(HWND_TOP),
                x,
                y,
                width,
                height,
                SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE,
            )
        };
        if let Err(e) = moved {
            eprintln!("[ourcamp] stream surface: {e}");
        }
    }

    /// Shows it when there is a picture; without one the page shows through.
    pub fn show(self, visible: bool) {
        let command = if visible { SW_SHOWNA } else { SW_HIDE };
        // SAFETY: as in `place`. The result says whether it was visible, not an error.
        let _ = unsafe { ShowWindowAsync(self.hwnd(), command) };
    }
}

type Handler = Box<dyn Fn(Input) + Send>;

const CLASS: PCWSTR = w!("OurCampStreamPicture");

fn instance() -> windows::core::Result<HMODULE> {
    // SAFETY: asks for this program's own module, which needs no release.
    unsafe { GetModuleHandleW(None) }
}

fn register_class() -> windows::core::Result<()> {
    static RESULT: OnceLock<windows::core::Result<()>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            let class = WNDCLASSEXW {
                cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).expect("a small struct"),
                // Without it Windows sends no `WM_LBUTTONDBLCLK`.
                style: CS_DBLCLKS,
                lpfnWndProc: Some(window_proc),
                hInstance: instance()?.into(),
                // SAFETY: stock objects are owned by the system and never released.
                hbrBackground: HBRUSH(unsafe { GetStockObject(BLACK_BRUSH) }.0),
                lpszClassName: CLASS,
                ..WNDCLASSEXW::default()
            };
            // SAFETY: the struct is complete and its name is a static string.
            let atom = unsafe { RegisterClassExW(&raw const class) };
            if atom == 0 {
                Err(windows::core::Error::from_thread())
            } else {
                Ok(())
            }
        })
        .clone()
}

/// Whether a point of the screen lies in the frame where the window is resized.
fn on_resize_border(hwnd: HWND, lparam: LPARAM) -> bool {
    // The low and high words of `lparam` are signed: screens to the left of the main are
    // negative.
    #[allow(clippy::cast_possible_truncation, reason = "taking the words apart")]
    let mut point = POINT {
        x: i32::from(lparam.0 as i16),
        y: i32::from((lparam.0 >> 16) as i16),
    };
    // SAFETY: the handles come from the system, for a message it is delivering.
    unsafe {
        let Ok(parent) = GetParent(hwnd) else {
            return false;
        };
        if IsZoomed(parent).as_bool() {
            return false;
        }
        if !ScreenToClient(parent, &raw mut point).as_bool() {
            return false;
        }
        let mut client = RECT::default();
        if GetClientRect(parent, &raw mut client).is_err() {
            return false;
        }
        point.x < RESIZE_BORDER
            || point.y < RESIZE_BORDER
            || point.x >= client.right - RESIZE_BORDER
            || point.y >= client.bottom - RESIZE_BORDER
    }
}

/// The child window's messages. The system calls it on the thread that made the window.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            // SAFETY: for this message `lparam` is the creation struct, and its parameter
            // is the handler `create` passed.
            unsafe {
                let create = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
        }
        WM_NCDESTROY => {
            // SAFETY: the pointer was put there at `WM_NCCREATE` and this is the last
            // message of the window, so it is taken back exactly once.
            unsafe {
                let handler = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut Handler;
                if !handler.is_null() {
                    drop(Box::from_raw(handler));
                }
            }
        }
        // Lets the edges of the window through to the handles that resize it.
        WM_NCHITTEST if on_resize_border(hwnd, lparam) => {
            return LRESULT(HTTRANSPARENT as isize);
        }
        WM_LBUTTONDOWN => {
            // SAFETY: gives this window, on its own thread, the keyboard for Esc.
            let _ = unsafe { SetFocus(Some(hwnd)) };
        }
        WM_LBUTTONDBLCLK => notify(hwnd, Input::DoubleClick),
        WM_KEYDOWN if wparam.0 == usize::from(VK_ESCAPE.0) => notify(hwnd, Input::Escape),
        _ => {}
    }
    // SAFETY: the arguments are the ones the system gave us.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn notify(hwnd: HWND, input: Input) {
    // SAFETY: set at `WM_NCCREATE`, cleared only at `WM_NCDESTROY`, on this same thread.
    unsafe {
        let handler = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Handler;
        if let Some(handler) = handler.as_ref() {
            handler(input);
        }
    }
}
