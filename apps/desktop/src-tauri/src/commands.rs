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

/// Kicks someone of a lower rank out of the open camp (ADR-0033).
#[tauri::command]
pub async fn camp_kick(
    state: State<'_, CampsState>,
    id: String,
    user_id: String,
) -> Result<(), String> {
    camps(&state)?.kick(&id, &user_id).await
}

/// Forbids someone of a lower rank to speak, or lifts it (ADR-0033).
#[tauri::command]
pub async fn camp_set_silenced(
    state: State<'_, CampsState>,
    id: String,
    user_id: String,
    silenced: bool,
) -> Result<(), String> {
    camps(&state)?.set_silenced(&id, &user_id, silenced).await
}

/// Your avatar on this device, base64url (ADR-0035); `None` without one.
#[tauri::command]
pub fn profile_avatar(state: State<'_, CampsState>) -> Result<Option<String>, String> {
    Ok(camps(&state)?.avatar().map(|b| oc_proto::encode_bytes(&b)))
}

/// Sets your avatar (base64url of a PNG/WebP), or drops it with `None`.
#[tauri::command]
pub async fn profile_set_avatar(
    state: State<'_, CampsState>,
    picture: Option<String>,
) -> Result<(), String> {
    let picture = picture
        .map(|p| oc_proto::decode_bytes(&p).ok_or_else(|| "Повреждённая картинка".to_owned()))
        .transpose()?;
    camps(&state)?.set_avatar(picture).await
}

/// A member's avatar by its hash, base64url (from the cache or the camp).
#[tauri::command]
pub async fn camp_avatar(
    state: State<'_, CampsState>,
    id: String,
    user_id: String,
    hash: String,
) -> Result<String, String> {
    camps(&state)?
        .member_avatar(&id, &user_id, &hash)
        .await
        .map(|b| oc_proto::encode_bytes(&b))
}

/// A page of a chat, oldest first (ADR-0044).
#[tauri::command]
pub async fn chat_history(
    state: State<'_, CampsState>,
    id: String,
    channel_id: String,
    before: Option<u64>,
    limit: u32,
) -> Result<Vec<oc_proto::ChatMessage>, String> {
    camps(&state)?
        .history(&id, &channel_id, before, limit)
        .await
}

/// Writes in a chat; `photo` (base64url, as the window encoded it) goes up first.
#[tauri::command]
pub async fn chat_send(
    state: State<'_, CampsState>,
    id: String,
    channel_id: String,
    text: String,
    photo: Option<OutgoingPhotoDto>,
) -> Result<oc_proto::ChatMessage, String> {
    let photo = photo
        .map(|p| {
            oc_proto::decode_bytes(&p.bytes)
                .map(|bytes| crate::camps::OutgoingPhoto {
                    bytes,
                    width: p.width,
                    height: p.height,
                })
                .ok_or_else(|| "Повреждённое фото".to_owned())
        })
        .transpose()?;
    camps(&state)?.send(&id, &channel_id, text, photo).await
}

/// A photo of the open camp's chats as it crosses IPC: base64url bytes and its size.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingPhotoDto {
    bytes: String,
    width: u32,
    height: u32,
}

/// A photo by its hash, base64url (from the cache or the camp).
#[tauri::command]
pub async fn chat_photo(
    state: State<'_, CampsState>,
    id: String,
    hash: String,
) -> Result<String, String> {
    camps(&state)?
        .photo(&id, &hash)
        .await
        .map(|b| oc_proto::encode_bytes(&b))
}

/// This device has read a chat up to `seq`; every window hears it.
#[tauri::command]
pub fn chat_mark_read(
    state: State<'_, CampsState>,
    id: String,
    channel_id: String,
    seq: u64,
) -> Result<(), String> {
    camps(&state)?.mark_read(&id, &channel_id, seq)
}

/// The open camp for the chat window: as opened, with its latest snapshot (ADR-0046).
#[tauri::command]
pub fn chat_context(
    state: State<'_, CampsState>,
) -> Result<Option<crate::camps::ChatContext>, String> {
    Ok(camps(&state)?.chat_context())
}

/// Opens the chat window beside the main one, or brings it forward. `async` is what makes
/// this work: a command without it runs on the main thread, and creating a window there
/// deadlocks on Windows (`WebView2`) — the whole app stops, tray and close button included.
#[tauri::command]
pub async fn chat_window_open(app: tauri::AppHandle) -> Result<(), String> {
    crate::windows::open_chat(&app).map_err(|e| format!("Не удалось открыть окно чата: {e}"))
}

/// The camp's friends link, to copy.
#[tauri::command]
pub async fn camp_invite(state: State<'_, CampsState>, id: String) -> Result<String, String> {
    camps(&state)?.invite_link(&id).await
}
