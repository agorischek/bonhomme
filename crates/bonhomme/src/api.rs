use crate::demo::{
    DemoMergeRun, DemoState, SpawnAgentsRequest, ensure_demo, merge_all_agents, merge_next_agent,
    reset_demo, spawn_agents,
};
use crate::simulation::{SimulationRequest, SimulationResult, run_simulation};
use anyhow::Result;
use bonhomme_engine::{DEFAULT_DATABASE_URL, MergeResult, Storage};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

#[derive(Clone)]
struct AppState {
    storage: Storage,
}

pub async fn serve(database_url: Option<String>, addr: SocketAddr) -> Result<()> {
    let database_url = database_url.unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
    let storage = Storage::connect(
        &database_url,
        std::sync::Arc::new(bonhomme_ts::TypeScriptPlugin),
    )
    .await?;
    storage.migrate().await?;

    let app = router(storage);
    let listener = TcpListener::bind(addr).await?;
    info!("bonhomme API listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(storage: Storage) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/demo/state", get(demo_state))
        .route("/api/demo/reset", post(reset))
        .route("/api/demo/spawn", post(spawn))
        .route("/api/demo/merge-next", post(merge_next))
        .route("/api/demo/merge/{branch}", post(merge_demo_branch))
        .route("/api/demo/merge-all", post(merge_all))
        .route("/api/demo/simulate", post(simulate))
        .route(
            "/api/repos/{repo}/branches/{branch}/render",
            get(render_branch),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(AppState { storage })
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        name: "bonhomme",
        status: "ok",
    })
}

async fn demo_state(State(state): State<AppState>) -> ApiResult<DemoState> {
    Ok(Json(ensure_demo(&state.storage).await?))
}

async fn reset(State(state): State<AppState>) -> ApiResult<DemoState> {
    Ok(Json(reset_demo(&state.storage).await?))
}

async fn spawn(
    State(state): State<AppState>,
    Json(request): Json<SpawnAgentsRequest>,
) -> ApiResult<DemoState> {
    Ok(Json(spawn_agents(&state.storage, request).await?))
}

async fn merge_next(State(state): State<AppState>) -> ApiResult<Option<MergeResult>> {
    Ok(Json(merge_next_agent(&state.storage).await?))
}

async fn merge_demo_branch(
    State(state): State<AppState>,
    Path(branch): Path<String>,
) -> ApiResult<MergeResult> {
    Ok(Json(
        state
            .storage
            .merge_branch(crate::demo::DEMO_REPOSITORY, &branch, "main")
            .await?,
    ))
}

async fn merge_all(State(state): State<AppState>) -> ApiResult<DemoMergeRun> {
    Ok(Json(merge_all_agents(&state.storage).await?))
}

async fn simulate(
    State(state): State<AppState>,
    Json(request): Json<SimulationRequest>,
) -> ApiResult<SimulationResult> {
    Ok(Json(run_simulation(&state.storage, request).await?))
}

async fn render_branch(
    State(state): State<AppState>,
    Path((repo, branch)): Path<(String, String)>,
) -> ApiResult<bonhomme_engine::MaterializedBranch> {
    Ok(Json(
        state.storage.materialize_branch(&repo, &branch).await?,
    ))
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

#[derive(Serialize)]
struct HealthResponse {
    name: &'static str,
    status: &'static str,
}

struct ApiError(anyhow::Error);

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Classify client-driven failures (missing repo/branch/symbol, unsupported merge shape) as
        // 4xx so they are not conflated with genuine server faults. The anyhow chain carries no
        // typed status, so we inspect its rendered message.
        let message = format!("{:#}", self.0);
        let status = if message.contains("does not exist") || message.contains("not found") {
            StatusCode::NOT_FOUND
        } else if message.contains("only supports") || message.contains("not supported") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        let body = Json(ErrorBody {
            error: self.0.to_string(),
        });
        (status, body).into_response()
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}
