//! Chats (M5, ADR-0044): history, writing, photos. Members only; everyone in the camp
//! reads and writes every chat, except those forbidden to speak, who only read.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use oc_proto::{
    ErrorCode, History, NewMessage, Photo, PhotoFormat, PhotoUploaded, MAX_HISTORY,
    MAX_MESSAGE_CHARS, MAX_PHOTO_BYTES, MAX_PHOTO_SIDE,
};
use serde::Deserialize;

use super::{manage_error, session_member, ApiError, AppState};
use crate::store::{Author, Posted};

/// How many messages a history request returns unless it asks.
const DEFAULT_HISTORY: u32 = 50;

/// `?before=&limit=` of a history request.
#[derive(Debug, Deserialize)]
pub(super) struct Page {
    /// Only messages with a smaller `seq`; none: up to the latest.
    before: Option<u64>,
    /// At most this many, 1–[`MAX_HISTORY`].
    limit: Option<u32>,
}

/// `GET /api/v1/channels/{id}/messages`: a page of a chat, oldest first.
pub(super) async fn history(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Query(page): Query<Page>,
    headers: HeaderMap,
) -> Result<Json<History>, ApiError> {
    session_member(&state, &headers).await?;
    let limit = page.limit.unwrap_or(DEFAULT_HISTORY).clamp(1, MAX_HISTORY);
    let messages = state
        .with_store(move |s| s.history(&channel_id, page.before, limit))
        .await?
        .ok_or(ApiError(ErrorCode::NotFound))?;
    Ok(Json(History { messages }))
}

/// Whether a message may be kept as it is: some text or a photo, within the limits.
fn acceptable(message: &NewMessage) -> bool {
    let chars = message.text.chars().count();
    let sides = |p: &Photo| {
        (1..=MAX_PHOTO_SIDE).contains(&p.width) && (1..=MAX_PHOTO_SIDE).contains(&p.height)
    };
    chars <= MAX_MESSAGE_CHARS
        && (chars > 0 || message.photo.is_some())
        && message.photo.as_ref().is_none_or(sides)
}

/// `POST /api/v1/channels/{id}/messages`: writes in a chat; everyone gets it as a
/// `message` event, the author too.
pub(super) async fn post_message(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    headers: HeaderMap,
    body: Result<Json<NewMessage>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<oc_proto::ChatMessage>, ApiError> {
    let member = session_member(&state, &headers).await?;
    let Json(mut message) = body?;
    // Trailing newlines and spaces are never meant; inner ones are.
    let kept = message.text.trim_end().len();
    message.text.truncate(kept);
    if !acceptable(&message) {
        return Err(ApiError(ErrorCode::BadRequest));
    }
    if member.silenced {
        return Err(ApiError(ErrorCode::Silenced));
    }
    let author = Author {
        user_id: &member.user_id,
        name: &member.name,
    };
    match state
        .manager
        .post_message(&channel_id, author, message)
        .await
        .map_err(manage_error)?
    {
        Posted::Message(sent) => Ok(Json(sent)),
        Posted::NoChat => Err(ApiError(ErrorCode::NotFound)),
        Posted::NoPhoto => Err(ApiError(ErrorCode::BadRequest)),
    }
}

/// `POST /api/v1/photos`: the photo itself as the body, before the message that shows
/// it. WebP, PNG or JPEG by its signature, at most [`MAX_PHOTO_BYTES`]; the camp never
/// decodes it, the apps' webviews do.
pub(super) async fn put_photo(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<PhotoUploaded>, ApiError> {
    let member = session_member(&state, &headers).await?;
    if member.silenced {
        return Err(ApiError(ErrorCode::Silenced));
    }
    let format = (body.len() <= MAX_PHOTO_BYTES)
        .then(|| PhotoFormat::sniff(&body))
        .flatten()
        .ok_or(ApiError(ErrorCode::BadRequest))?;
    let hash = state
        .with_store(move |s| s.put_photo(format, &body))
        .await?;
    Ok(Json(PhotoUploaded { hash }))
}

/// `GET /api/v1/photos/{hash}`: a photo, for members. It never changes under its hash,
/// so apps keep it.
pub(super) async fn photo(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    session_member(&state, &headers).await?;
    let (mime, data) = state
        .with_store(move |s| s.photo(&hash))
        .await?
        .ok_or(ApiError(ErrorCode::NotFound))?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_owned(),
            ),
        ],
        data,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str, photo: Option<(u32, u32)>) -> NewMessage {
        NewMessage {
            text: text.into(),
            photo: photo.map(|(width, height)| Photo {
                hash: "h".into(),
                width,
                height,
            }),
        }
    }

    #[test]
    fn a_message_is_some_text_or_a_photo_within_the_limits() {
        assert!(acceptable(&message("Привет", None)));
        assert!(acceptable(&message("", Some((1600, 900)))));
        assert!(!acceptable(&message("", None)), "nothing to say");
        assert!(
            acceptable(&message(&"я".repeat(MAX_MESSAGE_CHARS), None)),
            "characters, not bytes"
        );
        assert!(!acceptable(&message(
            &"я".repeat(MAX_MESSAGE_CHARS + 1),
            None
        )));
        assert!(!acceptable(&message("", Some((0, 10)))));
        assert!(!acceptable(&message("", Some((MAX_PHOTO_SIDE + 1, 10)))));
    }
}
