use crate::demo::{
    AgentSliceRequest, AgentSliceResponse, ApplyAgentSliceRequest, DemoMergeRun, DemoState,
    RealSwarmRequest, RealSwarmResponse, RealSwarmStatus, SpawnAgentsRequest, apply_agent_slice,
    create_agent_slice, ensure_demo, merge_all_agents, merge_next_agent, reset_demo, spawn_agents,
};
use crate::simulation::{SimulationRequest, SimulationResult, run_simulation};
use anyhow::{Context, Result, bail};
use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bonhomme_engine::{
    ConflictRecord, DEFAULT_DATABASE_URL, MergeResult, ResolveConflictResult, Storage,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    net::{IpAddr, SocketAddr},
    path::{Path as StdPath, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpListener, process::Command, sync::RwLock};
use tower::limit::ConcurrencyLimitLayer;
use tower_http::{
    catch_panic::CatchPanicLayer,
    limit::RequestBodyLimitLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    sensitive_headers::SetSensitiveHeadersLayer,
    timeout::TimeoutLayer,
    trace::TraceLayer,
};
use tracing::{error, info};
use uuid::Uuid;

mod jobs;

const SWARM_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "TERM",
    "CODEX_HOME",
    "CODEX_BIN",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

// The Node coordinator needs a credential to call this API. It must never reuse the service's
// tenant/database configuration, and the coordinator strips this token before spawning Codex.
const SWARM_API_ENV: &[&str] = &["BONHOMME_API_TOKEN"];
const NODE_PROBE_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
];

fn swarm_coordinator_environment(
    source: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    source
        .into_iter()
        .filter(|(key, _)| {
            key.to_str().is_some_and(|key| {
                SWARM_ENV_ALLOWLIST.contains(&key)
                    || SWARM_API_ENV.contains(&key)
                    || key.starts_with("LC_")
            })
        })
        .collect()
}

fn node_probe_environment(
    source: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    source
        .into_iter()
        .filter(|(key, _)| {
            key.to_str()
                .is_some_and(|key| NODE_PROBE_ENV_ALLOWLIST.contains(&key))
        })
        .collect()
}

async fn probe_node(
    program: &OsStr,
    environment: Vec<(OsString, OsString)>,
) -> Result<std::process::ExitStatus> {
    let mut command = Command::new(program);
    command
        .env_clear()
        .envs(environment)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
        .status()
        .await
        .context("failed to run node; install Node.js before launching a real swarm")
}

#[derive(Clone)]
struct AppState {
    storage: Storage,
    root: PathBuf,
    api_base: String,
    real_swarm: Arc<RwLock<RealSwarmStatus>>,
    metrics: crate::security::ApiMetrics,
}

struct RealSwarmLaunchGuard {
    status: Arc<RwLock<RealSwarmStatus>>,
    armed: bool,
}

impl RealSwarmLaunchGuard {
    fn new(status: Arc<RwLock<RealSwarmStatus>>) -> Self {
        Self {
            status,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RealSwarmLaunchGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Ok(mut status) = self.status.try_write() {
            mark_real_swarm_launch_cancelled(&mut status);
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let status = self.status.clone();
            runtime.spawn(async move {
                let mut status = status.write().await;
                mark_real_swarm_launch_cancelled(&mut status);
            });
        }
    }
}

fn mark_real_swarm_launch_cancelled(status: &mut RealSwarmStatus) {
    if status.running {
        status.running = false;
        status.error = Some("real swarm launch was cancelled".to_string());
    }
}

pub async fn serve(
    database_url: Option<String>,
    config: &crate::config::Config,
    root: &StdPath,
    addr: SocketAddr,
) -> Result<()> {
    let database_url = database_url.unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
    let storage = Storage::connect(
        &database_url,
        crate::plugins::language_registry(config, root),
    )
    .await?;
    storage.migrate().await?;

    let api_base = api_base_url(addr);
    let security = crate::security::ApiSecurity::from_environment(&config.server, addr)?;
    let control = crate::security::ApiControl::new(security, root).await?;
    let app = router(storage, root.to_path_buf(), api_base, control, config);
    let listener = TcpListener::bind(addr).await?;
    info!("bonhomme API listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(
    storage: Storage,
    root: PathBuf,
    api_base: String,
    control: crate::security::ApiControl,
    config: &crate::config::Config,
) -> Router {
    let metrics = crate::security::ApiMetrics::default();
    let protected = Router::new()
        .route("/api/whoami", get(whoami))
        .route("/api/metrics", get(api_metrics))
        .route("/api/demo/state", get(demo_state))
        .route("/api/demo/reset", post(reset))
        .route("/api/demo/spawn", post(spawn))
        .route("/api/demo/real-swarm", post(real_swarm))
        .route("/api/demo/real-swarm/status", get(real_swarm_status))
        .route("/api/demo/agent-slice", post(agent_slice))
        .route("/api/demo/apply-slice", post(apply_slice))
        .route("/api/demo/merge-next", post(merge_next))
        .route("/api/demo/merge/{branch}", post(merge_demo_branch))
        .route("/api/demo/merge-all", post(merge_all))
        .route("/api/demo/simulate", post(simulate))
        .route("/api/conflicts", get(list_conflicts))
        .route("/api/conflicts/{id}", get(show_conflict))
        .route("/api/conflicts/{id}/checkout", post(checkout_conflict))
        .route("/api/conflicts/{id}/resolve", post(resolve_conflict))
        .route("/api/conflicts/{id}/abandon", post(abandon_conflict))
        .route(
            "/api/repos/{repo}/branches/{branch}/render",
            get(render_branch),
        )
        .route(
            "/api/repos/{repo}/jobs",
            get(jobs::list_jobs).post(jobs::enqueue_job),
        )
        .route("/api/repos/{repo}/jobs/lease", post(jobs::lease_job))
        .route("/api/repos/{repo}/jobs/{job}", get(jobs::show_job))
        .route(
            "/api/repos/{repo}/jobs/{job}/heartbeat",
            post(jobs::heartbeat_job),
        )
        .route(
            "/api/repos/{repo}/jobs/{job}/succeed",
            post(jobs::succeed_job),
        )
        .route("/api/repos/{repo}/jobs/{job}/fail", post(jobs::fail_job))
        .route(
            "/api/repos/{repo}/jobs/{job}/cancel",
            post(jobs::cancel_job),
        )
        .route_layer(middleware::from_fn_with_state(
            control,
            crate::security::require_principal,
        ));
    let app = Router::new()
        .route("/api/health", get(health))
        .merge(protected);
    #[cfg(test)]
    let app = app
        .route("/__test/runtime/echo", post(runtime_test_echo))
        .route(
            "/__test/runtime/delay/{milliseconds}",
            get(runtime_test_delay),
        );
    app.layer(ConcurrencyLimitLayer::new(
        config.server.max_concurrent_requests,
    ))
    .layer(TimeoutLayer::with_status_code(
        StatusCode::GATEWAY_TIMEOUT,
        Duration::from_secs(config.server.request_timeout_seconds),
    ))
    .layer(RequestBodyLimitLayer::new(config.server.max_request_bytes))
    .layer(CatchPanicLayer::new())
    .layer(PropagateRequestIdLayer::x_request_id())
    .layer(TraceLayer::new_for_http())
    .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
    .layer(SetSensitiveHeadersLayer::new(std::iter::once(
        AUTHORIZATION,
    )))
    .layer(middleware::from_fn_with_state(
        metrics.clone(),
        crate::security::observe_requests,
    ))
    .with_state(AppState {
        storage,
        root,
        api_base,
        real_swarm: Arc::new(RwLock::new(RealSwarmStatus::default())),
        metrics,
    })
}

#[cfg(test)]
async fn runtime_test_echo(body: String) -> String {
    body
}

#[cfg(test)]
async fn runtime_test_delay(Path(milliseconds): Path<u64>) -> StatusCode {
    tokio::time::sleep(Duration::from_millis(milliseconds)).await;
    StatusCode::OK
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        name: "bonhomme",
        status: "ok",
    })
}

async fn whoami(
    Extension(principal): Extension<crate::security::Principal>,
) -> Json<crate::security::Principal> {
    Json(principal)
}

async fn api_metrics(State(state): State<AppState>) -> Json<crate::security::ApiMetricsSnapshot> {
    Json(state.metrics.snapshot())
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

async fn real_swarm(
    State(state): State<AppState>,
    Json(request): Json<RealSwarmRequest>,
) -> ApiResult<RealSwarmResponse> {
    let config = validate_real_swarm(&state, &request).await?;
    {
        let mut status = state.real_swarm.write().await;
        if status.running {
            return Err(anyhow::anyhow!("a real swarm is already running").into());
        }
        *status = RealSwarmStatus {
            running: true,
            ..RealSwarmStatus::default()
        };
    }
    let mut launch_guard = RealSwarmLaunchGuard::new(state.real_swarm.clone());

    let result = async {
        let state_after_reset = reset_demo(&state.storage).await?;
        let launch = launch_real_swarm(&state, config).await?;
        Ok::<_, anyhow::Error>((state_after_reset, launch))
    }
    .await;
    let (state_after_reset, launch) = match result {
        Ok(result) => result,
        Err(error) => {
            error!(error = %format!("{error:#}"), "failed to launch real swarm");
            let mut status = state.real_swarm.write().await;
            status.running = false;
            status.error = Some("real swarm launch failed; inspect server logs".to_string());
            launch_guard.disarm();
            return Err(error.into());
        }
    };
    launch_guard.disarm();

    Ok(Json(RealSwarmResponse {
        state: state_after_reset,
        pid: launch.pid,
        log_path: launch.log_path,
        model: launch.model,
        agents: launch.agents,
        concurrency: launch.concurrency,
        merge: launch.merge,
        dry_run: launch.dry_run,
    }))
}

async fn real_swarm_status(State(state): State<AppState>) -> Json<RealSwarmStatus> {
    Json(state.real_swarm.read().await.clone())
}

async fn agent_slice(
    State(state): State<AppState>,
    Json(request): Json<AgentSliceRequest>,
) -> ApiResult<AgentSliceResponse> {
    Ok(Json(create_agent_slice(&state.storage, request).await?))
}

async fn apply_slice(
    State(state): State<AppState>,
    Json(request): Json<ApplyAgentSliceRequest>,
) -> ApiResult<DemoState> {
    Ok(Json(apply_agent_slice(&state.storage, request).await?))
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

async fn list_conflicts(State(state): State<AppState>) -> ApiResult<Vec<ConflictRecord>> {
    let repository = state
        .storage
        .repository_by_name(crate::demo::DEMO_REPOSITORY)
        .await?;
    Ok(Json(state.storage.list_conflicts(repository.id).await?))
}

async fn show_conflict(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<ConflictRecord> {
    Ok(Json(state.storage.conflict_by_id(id).await?))
}

async fn checkout_conflict(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<ConflictCheckoutResponse> {
    let conflict = state.storage.conflict_by_id(id).await?;
    let views = state.storage.conflict_views(id).await?;
    let directory =
        crate::cli::conflict_workspace::checkout_conflict(&state.root, &views, None).await?;
    let conflict = if conflict.state.is_active() {
        state.storage.mark_conflict_resolving(id).await?
    } else {
        conflict
    };
    Ok(Json(ConflictCheckoutResponse {
        conflict,
        directory: crate::cli::conflict_workspace::display_directory(&state.root, &directory),
    }))
}

async fn resolve_conflict(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<ResolveConflictRequest>,
) -> ApiResult<ResolveConflictResult> {
    let directory = api_conflict_workspace_directory(id, request.dir.as_deref())?;
    validate_api_conflict_workspace(&state.root, id, &directory).await?;
    let checked_out =
        crate::cli::conflict_workspace::read_checked_out_conflict(&state.root, Some(&directory))
            .await?;
    if checked_out.manifest.id != id {
        return Err(anyhow::anyhow!(
            "checked-out conflict id {} does not match {id}",
            checked_out.manifest.id
        )
        .into());
    }
    Ok(Json(
        state
            .storage
            .resolve_conflict(
                id,
                checked_out.resolved_files,
                request.agent.as_deref().unwrap_or("agent"),
                request.accept_drop.unwrap_or(false),
            )
            .await?,
    ))
}

fn api_conflict_workspace_directory(id: Uuid, requested: Option<&StdPath>) -> Result<PathBuf> {
    let expected = PathBuf::from(".bonhomme")
        .join("conflicts")
        .join(id.to_string());
    let Some(requested) = requested else {
        return Ok(expected);
    };
    let requested = requested
        .to_str()
        .context("conflict workspace directory must be valid UTF-8")?;
    let normalized = bonhomme_core::safe_relative_path(requested)
        .context("invalid conflict workspace directory")?;
    let workspace = if normalized
        .file_name()
        .is_some_and(|name| name == "resolved")
    {
        normalized.parent().unwrap_or(&normalized)
    } else {
        &normalized
    };
    if workspace != expected {
        bail!(
            "conflict workspace directory must be {}",
            expected.display()
        );
    }
    Ok(normalized)
}

async fn validate_api_conflict_workspace(
    root: &StdPath,
    id: Uuid,
    directory: &StdPath,
) -> Result<()> {
    let expected_relative = PathBuf::from(".bonhomme")
        .join("conflicts")
        .join(id.to_string());
    let workspace_relative = if directory.file_name().is_some_and(|name| name == "resolved") {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };
    if workspace_relative != expected_relative {
        bail!(
            "conflict workspace directory must be {}",
            expected_relative.display()
        );
    }

    let canonical_root = tokio::fs::canonicalize(root)
        .await
        .context("failed to canonicalize repository root for conflict workspace")?;
    let canonical_workspace = tokio::fs::canonicalize(root.join(&expected_relative))
        .await
        .context("failed to canonicalize conflict workspace directory")?;
    if canonical_workspace != canonical_root.join(&expected_relative) {
        bail!("conflict workspace directory must not traverse symlinks");
    }
    let canonical_resolved = tokio::fs::canonicalize(canonical_workspace.join("resolved"))
        .await
        .context("failed to canonicalize resolved conflict workspace directory")?;
    if canonical_resolved != canonical_workspace.join("resolved") {
        bail!("conflict workspace resolved directory must not be a symlink");
    }
    Ok(())
}

async fn abandon_conflict(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(request): Json<AbandonConflictRequest>,
) -> ApiResult<ConflictRecord> {
    let conflict = state.storage.abandon_conflict(id).await?;
    if let Some(reason) = request.reason {
        info!("abandoned conflict {id}: {reason}");
    }
    Ok(Json(conflict))
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

#[derive(Serialize)]
struct HealthResponse {
    name: &'static str,
    status: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConflictCheckoutResponse {
    conflict: ConflictRecord,
    directory: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveConflictRequest {
    dir: Option<PathBuf>,
    agent: Option<String>,
    accept_drop: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AbandonConflictRequest {
    reason: Option<String>,
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
        } else if message.contains("only supports")
            || message.contains("not supported")
            || message.contains("invalid real swarm request")
            || message.contains("agent job tenant key")
            || message.contains("agent job kind")
            || message.contains("agent job idempotency key")
            || message.contains("agent job max_attempts")
            || message.contains("agent job retry_backoff_seconds")
            || message.contains("agent job max_backoff_seconds")
            || message.contains("agent job lease owner")
            || message.contains("agent job lease token")
            || message.contains("agent job lease duration")
            || message.contains("agent job error")
            || message.contains("leaseSeconds")
            || message.contains("conflict workspace directory")
        {
            StatusCode::BAD_REQUEST
        } else if message.contains("already running")
            || message.contains("already used by a different job request")
            || message.contains("has no active lease owned by")
            || message.contains("cannot be cancelled")
            || message.contains("checked-out conflict id")
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        let public_message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            error!(error = %message, "API request failed");
            "internal server error".to_string()
        } else {
            self.0.to_string()
        };
        let body = Json(ErrorBody {
            error: public_message,
        });
        (status, body).into_response()
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

struct RealSwarmLaunch {
    pid: Option<u32>,
    log_path: String,
    model: String,
    agents: usize,
    concurrency: usize,
    merge: bool,
    dry_run: bool,
}

struct RealSwarmConfig {
    agents: usize,
    concurrency: usize,
    include_conflicts: bool,
    model: String,
    merge: bool,
    dry_run: bool,
    demo_dir: PathBuf,
}

const MAX_REAL_SWARM_AGENTS: usize = 120;

async fn validate_real_swarm(
    state: &AppState,
    request: &RealSwarmRequest,
) -> Result<RealSwarmConfig> {
    let agents = request.agents.unwrap_or(30);
    if !(1..=MAX_REAL_SWARM_AGENTS).contains(&agents) {
        bail!("invalid real swarm request: agents must be between 1 and {MAX_REAL_SWARM_AGENTS}");
    }

    let concurrency = request.concurrency.unwrap_or(agents);
    if concurrency == 0 || concurrency > agents {
        bail!("invalid real swarm request: concurrency must be between 1 and the agent count");
    }

    let model = request
        .model
        .clone()
        .unwrap_or_else(|| "gpt-5.3-codex-spark".to_string());
    if model.is_empty()
        || model.starts_with('-')
        || !model.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        bail!("invalid real swarm request: model name contains unsupported characters");
    }

    let demo_dir = state.root.join("demo");
    let script_path = demo_dir.join("scripts").join("codex-swarm.mjs");
    if !script_path.is_file() {
        bail!(
            "real swarm script is unavailable at {}; demo scripts are missing from this deployment",
            script_path.display()
        );
    }
    let node_status = probe_node(
        OsStr::new("node"),
        node_probe_environment(std::env::vars_os()),
    )
    .await?;
    if !node_status.success() {
        bail!("node is unavailable; install Node.js before launching a real swarm");
    }

    Ok(RealSwarmConfig {
        agents,
        concurrency,
        include_conflicts: request.include_conflicts.unwrap_or(true),
        model,
        merge: request.merge.unwrap_or(true),
        dry_run: request.dry_run.unwrap_or(false),
        demo_dir,
    })
}

async fn launch_real_swarm(state: &AppState, config: RealSwarmConfig) -> Result<RealSwarmLaunch> {
    let RealSwarmConfig {
        agents,
        concurrency,
        include_conflicts,
        model,
        merge,
        dry_run,
        demo_dir,
    } = config;

    let log_dir = state
        .root
        .join(".bonhomme")
        .join("web-swarm")
        .join(run_id()?);
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create {}", log_dir.display()))?;
    secure_private_directory(&log_dir)?;
    let log_path = log_dir.join("runner.log");
    let mut log_options = OpenOptions::new();
    log_options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        log_options.mode(0o600);
    }
    let stdout = log_options
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    secure_private_file(&log_path)?;
    let stderr = stdout
        .try_clone()
        .with_context(|| format!("failed to clone {}", log_path.display()))?;

    let mut args = vec![
        "scripts/codex-swarm.mjs".to_string(),
        "--api-base".to_string(),
        state.api_base.clone(),
        "--agents".to_string(),
        agents.to_string(),
        "--concurrency".to_string(),
        concurrency.to_string(),
        "--model".to_string(),
        model.clone(),
        "--no-reset".to_string(),
        "--keep-workdir".to_string(),
    ];
    if !include_conflicts {
        args.push("--no-conflicts".to_string());
    }
    if merge {
        args.push("--merge".to_string());
    }
    if dry_run {
        args.push("--dry-run".to_string());
    }

    let mut command = Command::new("node");
    command.env_clear();
    command.envs(swarm_coordinator_environment(std::env::vars_os()));
    let mut child = command
        .args(args)
        .current_dir(&demo_dir)
        .env("BONHOMME_API_BASE", &state.api_base)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| "failed to start real Codex swarm process")?;
    let pid = child.id();
    let log_path_string = public_swarm_log_path(&state.root, &log_path)?;
    let log_path_for_task = log_path.display().to_string();
    {
        let mut status = state.real_swarm.write().await;
        status.pid = pid;
        status.log_path = Some(log_path_string.clone());
    }
    let real_swarm = state.real_swarm.clone();
    tokio::spawn(async move {
        let (exit_code, error_message) = match child.wait().await {
            Ok(status) if status.success() => {
                info!("real Codex swarm finished successfully; log={log_path_for_task}");
                (status.code(), None)
            }
            Ok(status) => {
                error!("real Codex swarm exited with {status}; log={log_path_for_task}");
                (
                    status.code(),
                    Some(format!("real Codex swarm exited with {status}")),
                )
            }
            Err(error) => {
                error!("failed waiting for real Codex swarm: {error}; log={log_path_for_task}");
                (
                    None,
                    Some("real swarm process monitoring failed; inspect server logs".to_string()),
                )
            }
        };
        let mut status = real_swarm.write().await;
        status.running = false;
        status.exit_code = exit_code;
        status.error = error_message;
    });

    Ok(RealSwarmLaunch {
        pid,
        log_path: log_path_string,
        model,
        agents,
        concurrency,
        merge,
        dry_run,
    })
}

#[cfg(unix)]
fn secure_private_directory(path: &StdPath) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to secure directory {}", path.display()))
}

#[cfg(not(unix))]
fn secure_private_directory(_path: &StdPath) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_private_file(path: &StdPath) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to secure file {}", path.display()))
}

#[cfg(not(unix))]
fn secure_private_file(_path: &StdPath) -> Result<()> {
    Ok(())
}

fn public_swarm_log_path(root: &StdPath, log_path: &StdPath) -> Result<String> {
    let relative = log_path.strip_prefix(root).with_context(|| {
        format!(
            "real swarm log path {} is outside repository root",
            log_path.display()
        )
    })?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn api_base_url(addr: SocketAddr) -> String {
    let host = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("http://{host}:{}", addr.port())
}

fn run_id() -> Result<String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX epoch")?
        .as_millis();
    Ok(format!("{millis}"))
}

#[cfg(test)]
mod tests;
