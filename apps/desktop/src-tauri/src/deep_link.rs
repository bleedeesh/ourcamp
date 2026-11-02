//! `ourcamp://` links from outside the app (ADR-0078). The system starts the app with the link
//! as an argument; a running copy gets a second launch's arguments from `single-instance`.
//!
//! The link is only kept until the window takes it, and the window only fills in the join
//! form: a link on a web page can bring the app forward, but joining takes a press.
//!
//! macOS hands links over in `RunEvent::Opened` instead: arrives in M8 step 5.

use std::sync::{Mutex, PoisonError};

use tauri::{AppHandle, Emitter, Manager};

/// A link is waiting. Must match `LINK_EVENT` in `src/engine/tauri-camp-api.ts`.
const LINK_EVENT: &str = "link://open";
/// Longer than any real link: an invite with a long domain and a fingerprint.
const MAX_LINK_CHARS: usize = 512;

/// The link that came last and that the window has not taken yet.
#[derive(Debug, Default)]
pub struct PendingLink(Mutex<Option<String>>);

impl PendingLink {
    /// From this launch's own arguments.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        Self(Mutex::new(in_args(args)))
    }

    /// The window asks when it starts and on [`LINK_EVENT`]: nothing is lost if it was not
    /// listening yet, and a link is never handled twice.
    pub fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn put(&self, link: String) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(link);
    }
}

/// A second launch's arguments reached the running copy.
pub fn deliver(app: &AppHandle, args: impl IntoIterator<Item = String>) {
    let Some(link) = in_args(args) else {
        return;
    };
    app.state::<PendingLink>().put(link);
    let _ = app.emit(LINK_EVENT, ());
}

/// The first argument that is an `ourcamp://` link. What it leads to is checked by the join form,
/// which can say what is wrong with it; here it only has to be one piece of plain text.
fn in_args(args: impl IntoIterator<Item = String>) -> Option<String> {
    args.into_iter().find(|arg| {
        let scheme = arg.get(.."ourcamp://".len());
        scheme.is_some_and(|s| s.eq_ignore_ascii_case("ourcamp://"))
            && arg.chars().count() <= MAX_LINK_CHARS
            && !arg.chars().any(|c| c.is_whitespace() || c.is_control())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(args: &[&str]) -> Option<String> {
        in_args(args.iter().map(|&a| a.to_owned()))
    }

    #[test]
    fn the_link_is_found_among_the_arguments() {
        let link = "ourcamp://192.0.2.15/join/7k2q9m4x8v1c3b5n?fp=abc";
        assert_eq!(found(&[link]).as_deref(), Some(link));
        assert_eq!(found(&["--flag", link, "other"]).as_deref(), Some(link));
        assert_eq!(
            found(&["OURCAMP://example.com"]).as_deref(),
            Some("OURCAMP://example.com"),
            "browsers may write a scheme in any case"
        );
    }

    #[test]
    fn nothing_else_counts() {
        assert_eq!(found(&[]), None);
        assert_eq!(found(&["ourcamp"]), None);
        assert_eq!(found(&["ourcamp:"]), None);
        assert_eq!(found(&["https://example.com"]), None);
        assert_eq!(found(&["C:\\Program Files\\OurCamp\\ourcamp.exe"]), None);
        assert_eq!(found(&["щщщщщщщщщщщщ"]), None, "not cut inside a letter");
        assert_eq!(found(&["ourcamp://a b"]), None);
        assert_eq!(found(&["ourcamp://a\nb"]), None);
        let long = format!("ourcamp://{}", "a".repeat(MAX_LINK_CHARS));
        assert_eq!(found(&[&long]), None);
    }

    #[test]
    fn a_link_is_taken_once_and_the_last_one_wins() {
        let pending = PendingLink::from_args(["ourcamp://first".to_owned()]);
        pending.put("ourcamp://second".to_owned());
        assert_eq!(pending.take().as_deref(), Some("ourcamp://second"));
        assert_eq!(pending.take(), None);
    }
}
