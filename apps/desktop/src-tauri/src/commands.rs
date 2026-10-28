//! Tauri commands called by `src/engine/tauri-engine.ts`.
//!
//! Names and argument shapes are the contract with the frontend; Tauri maps the JS
//! camelCase keys (`userId`) to these `snake_case` parameters.

// Tauri injects `State` and deserialized arguments by value.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use serde::Deserialize;
use tauri::State;

use crate::audio::engine::{Engine, MemberSpec};
use crate::camps::{CampList, CampPreview, Camps, OpenCamp};

/// Engine slot managed by Tauri: `Err` holds why audio is unavailable so every command
/// can report it instead of the app refusing to start.
pub type EngineState = Result<Engine, String>;

fn engine(state: &State<'_, EngineState>) -> Result<Engine, String> {
    state.inner().clone()
}

/// Camp list slot: `Err` holds why the app data directory is unusable.
pub type CampsState = Result<Arc<Camps>, String>;

fn camps(state: &State<'_, CampsState>) -> Result<Arc<Camps>, String> {
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

/// Joins a voice channel (leaving any previous one). Resolves once connected.
#[tauri::command]
pub async fn voice_join_channel(
    state: State<'_, EngineState>,
    camps_state: State<'_, CampsState>,
    camp_id: String,
    channel_id: String,
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
    let engine = engine(&state)?;
    let result = async {
        let access = if engine.needs_room() {
            Some(
                camps(&camps_state)?
                    .room_access(&camp_id, &channel_id)
                    .await?,
            )
        } else {
            None
        };
        engine.join_channel(access, &specs).await
    }
    .await;
    result.inspect_err(|e| eprintln!("[ourcamp] join {camp_id}/{channel_id}: {e}"))
}

/// Leaves the current channel.
#[tauri::command]
pub async fn voice_leave_channel(state: State<'_, EngineState>) -> Result<(), String> {
    engine(&state)?
        .leave_channel()
        .await
        .inspect_err(|e| eprintln!("[ourcamp] leave: {e}"))
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

// ---- camps (ADR-0020) ----

/// Joined camps and the name you go by.
#[tauri::command]
pub fn camp_list(state: State<'_, CampsState>) -> Result<CampList, String> {
    Ok(camps(&state)?.list())
}

/// What an `ourcamp://` link leads to, before joining.
#[tauri::command]
pub async fn camp_preview(
    state: State<'_, CampsState>,
    link: String,
) -> Result<CampPreview, String> {
    camps(&state)?.preview(&link).await
}

/// Joins the camp a link points to and opens it.
#[tauri::command]
pub async fn camp_join(
    state: State<'_, CampsState>,
    engine_state: State<'_, EngineState>,
    link: String,
    name: String,
) -> Result<OpenCamp, String> {
    let camps = camps(&state)?;
    let camp = camps.join(&link, &name).await?;
    camps.follow(&camp.id);
    if let Ok(engine) = engine(&engine_state) {
        engine.set_self_id(&camp.me.user_id);
    }
    Ok(camp)
}

/// Opens a joined camp (fresh channels, or the cached ones if it is offline).
#[tauri::command]
pub async fn camp_open(
    state: State<'_, CampsState>,
    engine_state: State<'_, EngineState>,
    id: String,
) -> Result<OpenCamp, String> {
    let camps = camps(&state)?;
    let camp = camps.open(&id).await?;
    camps.follow(&camp.id);
    if let Ok(engine) = engine(&engine_state) {
        engine.set_self_id(&camp.me.user_id);
    }
    Ok(camp)
}

/// Leaves a camp — first its voice, when it is the open one. `true`: the camp heard it;
/// `false`: it was unreachable and is only gone from this device.
#[tauri::command]
pub async fn camp_leave(
    camps_state: State<'_, CampsState>,
    engine_state: State<'_, EngineState>,
    id: String,
) -> Result<bool, String> {
    let camps = camps(&camps_state)?;
    if camps.is_current(&id) {
        if let Ok(engine) = engine(&engine_state) {
            if let Err(e) = engine.leave_channel().await {
                eprintln!("[ourcamp] leave voice: {e}");
            }
        }
    }
    camps.leave(&id).await
}

/// Moves someone sitting in voice to another campfire of the open camp (ADR-0029).
#[tauri::command]
pub async fn camp_move_member(
    state: State<'_, CampsState>,
    id: String,
    user_id: String,
    channel_id: String,
) -> Result<(), String> {
    camps(&state)?.move_member(&id, &user_id, &channel_id).await
}

/// The camp's friends link, to copy.
#[tauri::command]
pub async fn camp_invite(state: State<'_, CampsState>, id: String) -> Result<String, String> {
    camps(&state)?.invite_link(&id).await
}
