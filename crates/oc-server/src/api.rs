//! HTTP API of the camp (`/api/v1`). Clients reach it over the pinned TLS port.
//!
//! Step 1 of M3 exposes what needs no identity: the camp's public description and
//! whether an invite is still good. Signing in and room tokens arrive in step 2.

use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use oc_invite::InviteKind;
use serde::Serialize;

use crate::store::{canonical_token, Camp, Store, StoreResult};

/// Bumped on breaking changes; clients refuse servers they do not understand.
pub const API_VERSION: u32 = 1;

/// State shared by request handlers.
#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<Store>>,
}

impl AppState {
    /// Wraps the store.
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// Runs a store call on the blocking pool: SQLite I/O must not stall the async
    /// workers that serve every other connection.
    async fn with_store<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Store) -> StoreResult<R> + Send + 'static,
    ) -> Result<R, ApiError> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            // A panic mid-query cannot leave SQLite inconsistent (transactions roll back).
            let store = store.lock().unwrap_or_else(PoisonError::into_inner);
            f(&store)
        })
        .await
        .map_err(|_| ApiError::Internal)?
        .map_err(|e| {
            eprintln!("[oc-server] {e}");
            ApiError::Internal
        })
    }
}

/// Errors as the client sees them: a status and a stable machine-readable code.
#[derive(Debug)]
enum ApiError {
    NotFound,
    Internal,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        (status, Json(ErrorBody { error })).into_response()
    }
}

/// The API router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/camp", get(camp))
        .route("/api/v1/invites/{token}", get(invite))
        .with_state(state)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CampResponse {
    api_version: u32,
    #[serde(flatten)]
    camp: Camp,
}

async fn camp(State(state): State<AppState>) -> Result<Json<CampResponse>, ApiError> {
    let camp = state.with_store(Store::camp).await?;
    Ok(Json(CampResponse {
        api_version: API_VERSION,
        camp,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InviteResponse {
    kind: &'static str,
    usable: bool,
}

async fn invite(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<InviteResponse>, ApiError> {
    // Lenient like the link parser (case, look-alikes), then an exact lookup.
    let token = canonical_token(&token).ok_or(ApiError::NotFound)?;
    let status = state
        .with_store(move |s| s.invite_status(token.as_str()))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(InviteResponse {
        kind: match status.kind {
            InviteKind::Join => "join",
            InviteKind::ClaimAdmin => "claim",
        },
        usable: status.usable,
    }))
}
