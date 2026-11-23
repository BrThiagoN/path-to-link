use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, Request, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use serde_json::json;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::{AppState, CreateShareRequest};

pub fn router(state: AppState, shutdown_tx: tokio::sync::watch::Sender<bool>) -> Router {
    routes().layer(Extension(shutdown_tx)).with_state(state)
}

pub fn managed_router(
    state: AppState,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    instance: String,
) -> Router {
    routes()
        .route("/api/instance", get(move || async move { instance }))
        .layer(Extension(shutdown_tx))
        .with_state(state)
}

fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/f/{id}", get(serve_file).head(serve_file))
        .route("/api/shares", post(create_share).get(list_shares))
        .route("/api/shares/by-path", delete(revoke_share_by_path))
        .route("/api/shares/{id}", delete(revoke_share))
        .route("/api/shutdown", post(shutdown))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "path-to-link" }))
}

async fn create_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateShareRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "missing or invalid control token");
    }
    match state.create_share(request).await {
        Ok(share) => (StatusCode::CREATED, Json(share)).into_response(),
        Err(message) => error(StatusCode::UNPROCESSABLE_ENTITY, &message),
    }
}

async fn list_shares(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !authorized(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "missing or invalid control token");
    }
    Json(json!({ "shares": state.list_shares().await })).into_response()
}

async fn revoke_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !authorized(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "missing or invalid control token");
    }
    if state.revoke_share(&id).await {
        Json(json!({ "revoked": true, "id": id })).into_response()
    } else {
        error(StatusCode::NOT_FOUND, "share was not found or has expired")
    }
}

#[derive(Deserialize)]
struct RevokeByPathRequest {
    path: String,
}

async fn revoke_share_by_path(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RevokeByPathRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "missing or invalid control token");
    }
    if state
        .revoke_share_by_path(std::path::Path::new(&request.path))
        .await
    {
        Json(json!({ "revoked": true, "path": request.path })).into_response()
    } else {
        error(StatusCode::NOT_FOUND, "share for this path was not found")
    }
}

async fn shutdown(
    State(state): State<AppState>,
    Extension(shutdown_tx): Extension<tokio::sync::watch::Sender<bool>>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return error(StatusCode::UNAUTHORIZED, "missing or invalid control token");
    }
    let _ = shutdown_tx.send(true);
    Json(json!({ "stopping": true })).into_response()
}

#[derive(Deserialize)]
struct FileAccessQuery {
    token: Option<String>,
}

async fn serve_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FileAccessQuery>,
    request: Request<Body>,
) -> Response {
    let Some(token) = query.token else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(path) = state.resolve_file(&id, &token).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    match ServeFile::new(path).oneshot(request).await {
        Ok(response) => {
            let (parts, body) = response.into_parts();
            Response::from_parts(parts, Body::new(body))
        }
        Err(error) => {
            tracing::error!(error = %error, share_id = %id, "failed to serve shared file");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| state.is_control_token_valid(token))
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}
