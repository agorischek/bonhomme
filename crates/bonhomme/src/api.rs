use crate::demo::{
    DemoMergeRun, DemoState, SpawnAgentsRequest, ensure_demo, merge_all_agents, merge_next_agent,
    reset_demo, spawn_agents,
};
use crate::simulation::{SimulationRequest, SimulationResult, run_simulation};
use crate::storage::{DEFAULT_DATABASE_URL, MergeResult, Storage};
use anyhow::Result;
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
    let storage = Storage::connect(&database_url).await?;
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
) -> ApiResult<crate::storage::MaterializedBranch> {
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
        let body = Json(ErrorBody {
            error: self.0.to_string(),
        });
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}
