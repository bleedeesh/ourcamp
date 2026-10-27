//! Tauri commands called by `src/engine/tauri-engine.ts`.
//!
//! Names and argument shapes are the contract with the frontend; Tauri maps the JS
//! camelCase keys (`userId`) to these snake_case parameters.

// Tauri injects `State` and deserialized arguments by value.
#![allow(clippy::needless_pass_by_value)]

use serde::Deserialize;
use tauri::State;

use crate::audio::engine::{Engine, MemberSpec};

/// Engine slot managed by Tauri: `Err` holds why audio is unavailable so every command
/// can report it instead of the app refusing to start.
pub type EngineState = Result<Engine, String>;

fn engine(state: &State<'_, EngineState>) -> Result<Engine, String> {
    state.inner().clone()
}

/// One remote member in a join request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinMember {
    user_id: String,
    volume: f32,
    muted: bool,
    /// The member's own microphone state (they send no audio while muted).
    #[serde(default)]
    mic_muted: bool,
}

/// Starts playing the given members (replacing any previous channel).
#[tauri::command]
pub fn voice_join_channel(
    state: State<'_, EngineState>,
    members: Vec<JoinMember>,
) -> Result<(), String> {
    let specs: Vec<MemberSpec> = members
        .into_iter()
        .map(|m| MemberSpec {
            user_id: m.user_id,
            volume_percent: m.volume,
            muted: m.muted,
            remote_mic_muted: m.mic_muted,
        })
        .collect();
    engine(&state)?.join_channel(&specs)
}

/// Stops playing the current channel.
#[tauri::command]
pub fn voice_leave_channel(state: State<'_, EngineState>) -> Result<(), String> {
    engine(&state)?.leave_channel()
}

/// Sets a member's local volume (0–200 %).
#[tauri::command]
pub fn voice_set_volume(
    state: State<'_, EngineState>,
    user_id: String,
    percent: f32,
) -> Result<(), String> {
    engine(&state)?.set_volume(&user_id, percent)
}

/// Mutes or unmutes a member for this listener only.
#[tauri::command]
pub fn voice_set_local_mute(
    state: State<'_, EngineState>,
    user_id: String,
    muted: bool,
) -> Result<(), String> {
    engine(&state)?.set_local_mute(&user_id, muted)
}

/// Mutes or unmutes the local microphone.
#[tauri::command]
pub fn voice_set_mic_muted(state: State<'_, EngineState>, muted: bool) -> Result<(), String> {
    engine(&state)?.set_mic_muted(muted);
    Ok(())
}

/// Turns all incoming sound off or on.
#[tauri::command]
pub fn voice_set_deafened(state: State<'_, EngineState>, deafened: bool) -> Result<(), String> {
    engine(&state)?.set_deafened(deafened)
}
