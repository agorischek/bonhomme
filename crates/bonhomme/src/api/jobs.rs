use super::{ApiResult, AppState};
use crate::security::Principal;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use bonhomme_engine::{AgentJob, AgentJobSpec, LeasedAgentJob};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

const DEFAULT_LEASE_SECONDS: u64 = 60;
const MAX_LEASE_SECONDS: u64 = 3_600;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EnqueueJobRequest {
    kind: String,
    idempotency_key: String,
    payload: Value,
    #[serde(default = "default_max_attempts")]
    max_attempts: i64,
    #[serde(default = "default_retry_backoff_seconds")]
    retry_backoff_seconds: i64,
    #[serde(default = "default_max_backoff_seconds")]
    max_backoff_seconds: i64,
    #[serde(default)]
    available_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LeaseJobRequest {
    owner: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default = "default_lease_seconds")]
    lease_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HeartbeatJobRequest {
    owner: String,
    lease_token: String,
    #[serde(default = "default_lease_seconds")]
    lease_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SucceedJobRequest {
    owner: String,
    lease_token: String,
    result: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FailJobRequest {
    owner: String,
    lease_token: String,
    error: String,
}

pub(super) async fn enqueue_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(repo): Path<String>,
    Json(request): Json<EnqueueJobRequest>,
) -> ApiResult<AgentJob> {
    let repository = state.storage.repository_by_name(&repo).await?;
    let spec = AgentJobSpec {
        tenant_key: principal.tenant,
        kind: request.kind,
        idempotency_key: request.idempotency_key,
        payload: request.payload,
        max_attempts: request.max_attempts,
        retry_backoff_seconds: request.retry_backoff_seconds,
        max_backoff_seconds: request.max_backoff_seconds,
        available_at: request.available_at,
    };
    Ok(Json(
        state.storage.enqueue_agent_job(repository.id, spec).await?,
    ))
}

pub(super) async fn list_jobs(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(repo): Path<String>,
) -> ApiResult<Vec<AgentJob>> {
    let repository = state.storage.repository_by_name(&repo).await?;
    Ok(Json(
        state
            .storage
            .list_agent_jobs(&principal.tenant, repository.id)
            .await?,
    ))
}

pub(super) async fn show_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((repo, job)): Path<(String, Uuid)>,
) -> ApiResult<AgentJob> {
    let repository = state.storage.repository_by_name(&repo).await?;
    Ok(Json(
        state
            .storage
            .agent_job_by_id(&principal.tenant, repository.id, job)
            .await?,
    ))
}

pub(super) async fn lease_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(repo): Path<String>,
    Json(request): Json<LeaseJobRequest>,
) -> ApiResult<Option<LeasedAgentJob>> {
    let repository = state.storage.repository_by_name(&repo).await?;
    Ok(Json(
        state
            .storage
            .lease_agent_job(
                &principal.tenant,
                repository.id,
                &request.owner,
                request.kind.as_deref(),
                lease_duration(request.lease_seconds)?,
            )
            .await?,
    ))
}

pub(super) async fn heartbeat_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((repo, job)): Path<(String, Uuid)>,
    Json(request): Json<HeartbeatJobRequest>,
) -> ApiResult<AgentJob> {
    let repository = state.storage.repository_by_name(&repo).await?;
    Ok(Json(
        state
            .storage
            .heartbeat_agent_job(
                &principal.tenant,
                repository.id,
                job,
                &request.owner,
                &request.lease_token,
                lease_duration(request.lease_seconds)?,
            )
            .await?,
    ))
}

pub(super) async fn succeed_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((repo, job)): Path<(String, Uuid)>,
    Json(request): Json<SucceedJobRequest>,
) -> ApiResult<AgentJob> {
    let repository = state.storage.repository_by_name(&repo).await?;
    Ok(Json(
        state
            .storage
            .succeed_agent_job(
                &principal.tenant,
                repository.id,
                job,
                &request.owner,
                &request.lease_token,
                request.result,
            )
            .await?,
    ))
}

pub(super) async fn fail_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((repo, job)): Path<(String, Uuid)>,
    Json(request): Json<FailJobRequest>,
) -> ApiResult<AgentJob> {
    let repository = state.storage.repository_by_name(&repo).await?;
    Ok(Json(
        state
            .storage
            .fail_agent_job(
                &principal.tenant,
                repository.id,
                job,
                &request.owner,
                &request.lease_token,
                &request.error,
            )
            .await?,
    ))
}

pub(super) async fn cancel_job(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path((repo, job)): Path<(String, Uuid)>,
) -> ApiResult<AgentJob> {
    let repository = state.storage.repository_by_name(&repo).await?;
    Ok(Json(
        state
            .storage
            .cancel_agent_job(&principal.tenant, repository.id, job)
            .await?,
    ))
}

fn lease_duration(seconds: u64) -> anyhow::Result<Duration> {
    if !(1..=MAX_LEASE_SECONDS).contains(&seconds) {
        anyhow::bail!("leaseSeconds must be between 1 and {MAX_LEASE_SECONDS}");
    }
    Ok(Duration::from_secs(seconds))
}

const fn default_lease_seconds() -> u64 {
    DEFAULT_LEASE_SECONDS
}

const fn default_max_attempts() -> i64 {
    3
}

const fn default_retry_backoff_seconds() -> i64 {
    1
}

const fn default_max_backoff_seconds() -> i64 {
    300
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_duration_is_bounded() {
        assert_eq!(lease_duration(1).unwrap(), Duration::from_secs(1));
        assert_eq!(
            lease_duration(MAX_LEASE_SECONDS).unwrap(),
            Duration::from_secs(MAX_LEASE_SECONDS)
        );
        assert!(lease_duration(0).is_err());
        assert!(lease_duration(MAX_LEASE_SECONDS + 1).is_err());
    }

    #[test]
    fn enqueue_payload_cannot_override_tenant() {
        let result = serde_json::from_value::<EnqueueJobRequest>(serde_json::json!({
            "tenantKey": "attacker",
            "kind": "demo",
            "idempotencyKey": "job-1",
            "payload": {}
        }));
        assert!(result.is_err());
    }
}
