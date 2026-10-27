//! Forwards "who is in the channel" to the UI (ADR-0011).

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::audio::engine::{Engine, Participants};

/// Must match `PARTICIPANTS_EVENT` in `src/engine/tauri-engine.ts`.
pub const PARTICIPANTS_EVENT: &str = "voice://participants";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MemberDto {
    user_id: String,
    name: String,
    mic_muted: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ParticipantsDto {
    self_id: String,
    members: Vec<MemberDto>,
}

impl From<Participants> for ParticipantsDto {
    fn from(p: Participants) -> Self {
        Self {
            self_id: p.self_id,
            members: p
                .members
                .into_iter()
                .map(|m| MemberDto {
                    user_id: m.user_id,
                    name: m.name,
                    mic_muted: m.mic_muted,
                })
                .collect(),
        }
    }
}

/// Sends every participants update from `engine` to the webview.
pub fn forward(app: AppHandle, engine: &Engine) {
    engine.set_participants_listener(Arc::new(move |participants| {
        if let Err(e) = app.emit(PARTICIPANTS_EVENT, ParticipantsDto::from(participants)) {
            eprintln!("[ourcamp] participants event: {e}");
        }
    }));
}
