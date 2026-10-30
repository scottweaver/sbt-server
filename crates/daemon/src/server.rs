//! The HTTP surface: `GET /health`, `GET /status`, `POST /exec` (an NDJSON
//! stream of `ExecEvent`s) and `POST /shutdown`. Request and response
//! shapes are the DTOs in `sbt_server_core::protocol`.

use std::sync::Arc;
use std::time::Instant;

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{StatusCode, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sbt_server_core::auth::BearerToken;
use sbt_server_core::instance::{CommandLine, StartError};
use sbt_server_core::protocol::{
    ErrorCode, ErrorResponse, ExecEvent, ExecRequest, HealthResponse, HealthStatus,
    ShutdownRequest, ShutdownResponse, StatusResponse,
};
use sbt_server_core::registry::Registry;
use sbt_server_core::routing::{BuildRoot, RoutingError, resolve_build_root};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

pub struct AppState {
    pub registry: Arc<Registry>,
    pub started_at: Instant,
}

/// `/health` is open; every other route requires the bearer token. The
/// loopback check is the outer layer, so a foreign `Host` is refused with
/// 403 before the token is looked at.
pub fn router(state: Arc<AppState>, token: BearerToken) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/exec", post(exec))
        .route("/shutdown", post(shutdown))
        .route_layer(from_fn_with_state(
            Arc::new(token),
            crate::guard::require_bearer_token,
        ))
        .route("/health", get(health))
        .layer(from_fn(crate::guard::require_loopback_client))
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: HealthStatus::Ok,
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_secs: state.started_at.elapsed().as_secs(),
    })
}

async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    Json(StatusResponse {
        instances: state.registry.status(),
    })
}

async fn exec(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ExecRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = body.map_err(|rejection| ApiError::rejected(&rejection))?;
    let root = route(&request.cwd)?;
    let command = CommandLine::new(request.command_line).map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::BadRequest,
            error.to_string(),
        )
    })?;
    let instance = state
        .registry
        .get_or_start(root)
        .await
        .map_err(|error| ApiError::start(&error))?;
    let events = instance.exec(command, request.verbose).into_receiver();
    let body = Body::from_stream(ReceiverStream::new(events).map(|event| ndjson_line(&event)));
    Ok(([(header::CONTENT_TYPE, "application/x-ndjson")], body).into_response())
}

fn ndjson_line(event: &ExecEvent) -> Result<Bytes, serde_json::Error> {
    let mut line = serde_json::to_vec(&event)?;
    line.push(b'\n');
    Ok(Bytes::from(line))
}

async fn shutdown(
    State(state): State<Arc<AppState>>,
    body: Result<Json<ShutdownRequest>, JsonRejection>,
) -> Result<Json<ShutdownResponse>, ApiError> {
    let Json(request) = body.map_err(|rejection| ApiError::rejected(&rejection))?;
    let stopped = match request {
        ShutdownRequest::All => state
            .registry
            .shutdown_all()
            .await
            .into_iter()
            .map(|(root, _)| root.as_path().to_path_buf())
            .collect(),
        ShutdownRequest::One { cwd } => {
            let root = route(&cwd)?;
            match state.registry.shutdown(&root).await {
                Some(_) => vec![root.as_path().to_path_buf()],
                None => Vec::new(),
            }
        }
    };
    Ok(Json(ShutdownResponse { stopped }))
}

fn route(cwd: &std::path::Path) -> Result<BuildRoot, ApiError> {
    resolve_build_root(cwd).map_err(|error| {
        let code = match &error {
            RoutingError::NoBuildRoot { .. } => ErrorCode::NoBuildRoot,
            RoutingError::Io { .. } => ErrorCode::BadRequest,
        };
        ApiError::new(StatusCode::BAD_REQUEST, code, error.to_string())
    })
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    body: ErrorResponse,
}

impl ApiError {
    fn new(status: StatusCode, code: ErrorCode, message: String) -> Self {
        Self {
            status,
            body: ErrorResponse {
                code,
                message,
                output: None,
            },
        }
    }

    /// A body axum could not turn into the request type, reported in
    /// sbtd's own error shape instead of axum's plain-text 422.
    fn rejected(rejection: &JsonRejection) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::BadRequest,
            format!("request body: {}", rejection.body_text()),
        )
    }

    pub(crate) fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "missing or wrong bearer token; sbtctl reads it from sbtd.token in the state dir"
                .to_owned(),
        )
    }

    pub(crate) fn forbidden(refusal: &crate::guard::Refusal) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            ErrorCode::Forbidden,
            refusal.to_string(),
        )
    }

    fn start(error: &StartError) -> Self {
        let code = match error {
            StartError::ProjectLoadFailed { .. } | StartError::BootTimeout { .. } => {
                ErrorCode::ProjectLoadFailed
            }
            StartError::Spawn { .. }
            | StartError::Connect { .. }
            | StartError::ConnectTimeout { .. }
            | StartError::Handshake(_)
            | StartError::HandshakeTimeout { .. }
            | StartError::UntrustedDescriptor(_)
            | StartError::Vanished => ErrorCode::StartFailed,
        };
        let output = error.output();
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ErrorResponse {
                code,
                message: error.to_string(),
                output: (!output.is_empty()).then(|| output.to_vec()),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}
