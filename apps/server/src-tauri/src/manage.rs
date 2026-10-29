//! Window commands for the owner's changes (ADR-0026): the camp's name, roles, channels,
//! and moderation (ADR-0028): kick, move, silence.
//! Each one calls `oc_server::Manager`; the result shows up through `server://status`
//! like any other change, so the commands return nothing but errors.

// Tauri injects `State` and deserialized arguments by value.
#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use oc_proto::{ChannelKind, Role};
use oc_server::{ManageError, Manager};
use tauri::State;

use crate::Host;

/// A refusal in words for the window.
fn describe(e: &ManageError) -> String {
    match e {
        ManageError::NotFound => "Такого участника или канала уже нет".into(),
        ManageError::BadName(max) => format!("Название — от 1 до {max} символов"),
        ManageError::AfkIsPermanent => {
            "Канал AFK удалить нельзя — его можно только переименовать".into()
        }
        ManageError::Store(e) => format!("Не удалось сохранить: {e}"),
        ManageError::RoomNotClosed(e) => {
            format!("Канал удалён, но голосовой сервер не закрыл его комнату: {e}")
        }
        ManageError::NotInVoice => "Участник уже не в голосовом канале".into(),
        ManageError::NotACampfire => "Перенести можно только в голосовой канал".into(),
        ManageError::LiveKit(e) => {
            format!("Сохранено, но голосовой сервер не применил это сейчас: {e}")
        }
    }
}

async fn manager(host: &Arc<Host>) -> Result<Manager, String> {
    host.manager()
        .await
        .ok_or_else(|| "Сервер не запущен".to_owned())
}

#[tauri::command]
pub async fn server_rename_camp(host: State<'_, Arc<Host>>, name: String) -> Result<(), String> {
    manager(&host)
        .await?
        .rename_camp(&name)
        .await
        .map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn server_set_role(
    host: State<'_, Arc<Host>>,
    user_id: String,
    role: Role,
) -> Result<(), String> {
    manager(&host)
        .await?
        .set_role(&user_id, role)
        .await
        .map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn server_create_channel(
    host: State<'_, Arc<Host>>,
    name: String,
    kind: ChannelKind,
) -> Result<(), String> {
    manager(&host)
        .await?
        .create_channel(&name, kind)
        .await
        .map(drop)
        .map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn server_rename_channel(
    host: State<'_, Arc<Host>>,
    id: String,
    name: String,
) -> Result<(), String> {
    manager(&host)
        .await?
        .rename_channel(&id, &name)
        .await
        .map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn server_delete_channel(host: State<'_, Arc<Host>>, id: String) -> Result<(), String> {
    manager(&host)
        .await?
        .delete_channel(&id)
        .await
        .map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn server_kick(host: State<'_, Arc<Host>>, user_id: String) -> Result<(), String> {
    manager(&host)
        .await?
        .kick(&user_id)
        .await
        .map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn server_move_member(
    host: State<'_, Arc<Host>>,
    user_id: String,
    channel_id: String,
) -> Result<(), String> {
    manager(&host)
        .await?
        .move_member(&user_id, &channel_id)
        .await
        .map_err(|e| describe(&e))
}

#[tauri::command]
pub async fn server_set_silenced(
    host: State<'_, Arc<Host>>,
    user_id: String,
    silenced: bool,
) -> Result<(), String> {
    manager(&host)
        .await?
        .set_silenced(&user_id, silenced)
        .await
        .map_err(|e| describe(&e))
}

/// A member's avatar for the window (ADR-0035): media type and base64url bytes.
#[derive(serde::Serialize)]
pub struct AvatarDto {
    mime: String,
    data: String,
}

#[tauri::command]
pub async fn server_avatar(
    host: State<'_, Arc<Host>>,
    user_id: String,
) -> Result<Option<AvatarDto>, String> {
    let avatar = manager(&host)
        .await?
        .avatar(&user_id)
        .await
        .map_err(|e| describe(&e))?;
    Ok(avatar.map(|(mime, bytes)| AvatarDto {
        mime,
        data: oc_proto::encode_bytes(&bytes),
    }))
}

/// Takes a member's avatar down (an unfitting picture); they may send another.
#[tauri::command]
pub async fn server_clear_avatar(
    host: State<'_, Arc<Host>>,
    user_id: String,
) -> Result<(), String> {
    manager(&host)
        .await?
        .clear_avatar(&user_id)
        .await
        .map_err(|e| describe(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_read_as_sentences() {
        assert_eq!(
            describe(&ManageError::BadName(32)),
            "Название — от 1 до 32 символов"
        );
        assert!(describe(&ManageError::AfkIsPermanent).contains("AFK"));
        assert!(describe(&ManageError::NotInVoice).contains("не в голосовом"));
    }
}
