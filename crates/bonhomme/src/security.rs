use crate::config::ServerConfig;
use anyhow::{Context, Result, bail};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncWriteExt,
    sync::Mutex,
};
use uuid::Uuid;

const TENANT_HEADER: &str = "x-bonhomme-tenant";

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApiRole {
    Reader,
    Writer,
    Admin,
}

impl ApiRole {
    fn from_env(value: Option<String>) -> Result<Self> {
        match value.as_deref().unwrap_or("admin") {
            "reader" => Ok(Self::Reader),
            "writer" => Ok(Self::Writer),
            "admin" => Ok(Self::Admin),
            value => bail!("BONHOMME_API_ROLE must be reader, writer, or admin; got {value}"),
        }
    }

    fn allows(self, method: &Method) -> bool {
        method == Method::GET
            || method == Method::HEAD
            || matches!(self, Self::Writer | Self::Admin)
    }
}

impl std::fmt::Display for ApiRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Reader => "reader",
            Self::Writer => "writer",
            Self::Admin => "admin",
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Principal {
    pub subject: String,
    pub tenant: String,
    pub role: ApiRole,
}

#[derive(Clone)]
pub(crate) struct ApiSecurity {
    token_digest: Option<[u8; 32]>,
    subject: String,
    role: ApiRole,
    tenant: String,
}

#[derive(Clone)]
pub(crate) struct ApiControl {
    security: ApiSecurity,
    audit: AuditLog,
}

impl ApiControl {
    pub(crate) async fn new(security: ApiSecurity, root: &Path) -> Result<Self> {
        Ok(Self {
            security,
            audit: AuditLog::open(&root.join(".bonhomme").join("audit.jsonl")).await?,
        })
    }
}

#[derive(Clone)]
struct AuditLog {
    state: Arc<Mutex<AuditLogState>>,
    path: PathBuf,
    lock_path: PathBuf,
}

struct AuditLogState {
    file: File,
    sequence: u64,
    previous_hash: String,
    observed_len: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditPayload {
    sequence: u64,
    timestamp_millis: u128,
    subject: String,
    tenant: String,
    role: String,
    method: String,
    path: String,
    request_id: Option<String>,
    phase: String,
    status: Option<u16>,
    previous_hash: String,
}

#[derive(Clone, Debug)]
struct AuditIdentity {
    subject: String,
    tenant: String,
    role: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct AuditEntry {
    payload: AuditPayload,
    hash: String,
}

impl AuditLog {
    async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let lock_path = path.with_extension("jsonl.lock");
        let _lock = acquire_exclusive_audit_lock(&lock_path).await?;
        let contents = match tokio::fs::read_to_string(path).await {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let (sequence, previous_hash) = verify_audit_chain(&contents)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(Self {
            state: Arc::new(Mutex::new(AuditLogState {
                file,
                sequence,
                previous_hash,
                observed_len: contents.len() as u64,
            })),
            path: path.to_path_buf(),
            lock_path,
        })
    }

    async fn record(
        &self,
        principal: &Principal,
        request: &AuditRequest,
        phase: &str,
        status: Option<StatusCode>,
    ) -> Result<()> {
        self.record_identity(
            &AuditIdentity {
                subject: principal.subject.clone(),
                tenant: principal.tenant.clone(),
                role: principal.role.to_string(),
            },
            request,
            phase,
            status,
        )
        .await
    }

    async fn record_identity(
        &self,
        identity: &AuditIdentity,
        request: &AuditRequest,
        phase: &str,
        status: Option<StatusCode>,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let _lock = acquire_exclusive_audit_lock(&self.lock_path).await?;
        let current_len = match tokio::fs::metadata(&self.path).await {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        if current_len != state.observed_len {
            let contents = tokio::fs::read_to_string(&self.path).await?;
            let (sequence, previous_hash) = verify_audit_chain(&contents)?;
            state.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .await?;
            state.sequence = sequence;
            state.previous_hash = previous_hash;
            state.observed_len = contents.len() as u64;
        }
        let payload = AuditPayload {
            sequence: state.sequence + 1,
            timestamp_millis: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
            subject: identity.subject.clone(),
            tenant: identity.tenant.clone(),
            role: identity.role.clone(),
            method: request.method.clone(),
            path: request.path.clone(),
            request_id: request.request_id.clone(),
            phase: phase.to_string(),
            status: status.map(|status| status.as_u16()),
            previous_hash: state.previous_hash.clone(),
        };
        let entry = AuditEntry {
            hash: audit_hash(&payload)?,
            payload,
        };
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        state.file.write_all(&line).await?;
        state.file.flush().await?;
        state.file.sync_data().await?;
        state.sequence = entry.payload.sequence;
        state.previous_hash = entry.hash;
        state.observed_len += line.len() as u64;
        Ok(())
    }
}

async fn acquire_exclusive_audit_lock(path: &Path) -> Result<std::fs::File> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok::<_, std::io::Error>(file)
    })
    .await
    .context("joining audit lock task")?
    .context("locking audit chain")
}

async fn acquire_existing_shared_audit_lock(path: &Path) -> Result<Option<std::fs::File>> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = match std::fs::OpenOptions::new().read(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        fs2::FileExt::lock_shared(&file)?;
        Ok::<_, std::io::Error>(Some(file))
    })
    .await
    .context("joining audit lock task")?
    .context("locking audit chain for verification")
}

/// One fail-closed audit scope for a direct CLI mutation. Only a static command classification is
/// recorded: argument values, storage URLs, paths, payloads, and environment secrets never enter the
/// audit record. An intent is durably appended before dispatch and an outcome after dispatch.
pub(crate) struct CliMutationAudit {
    audit: AuditLog,
    identity: AuditIdentity,
    request: AuditRequest,
}

impl CliMutationAudit {
    pub(crate) async fn begin(root: &Path, action: &'static str) -> Result<Self> {
        let identity = cli_audit_identity(
            std::env::var("BONHOMME_CLI_ACTOR").ok(),
            std::env::var("BONHOMME_CLI_TENANT").ok(),
            std::env::var("BONHOMME_API_SUBJECT").ok(),
            std::env::var("BONHOMME_API_TENANTS").ok(),
        )?;
        Self::begin_with_identity(root, action, identity).await
    }

    async fn begin_with_identity(
        root: &Path,
        action: &'static str,
        identity: AuditIdentity,
    ) -> Result<Self> {
        validate_cli_action(action)?;
        let audit = AuditLog::open(&root.join(".bonhomme").join("audit.jsonl")).await?;
        let request = AuditRequest {
            method: "CLI".to_string(),
            path: format!("/cli/{action}"),
            request_id: Some(format!("cli-{}", Uuid::new_v4())),
        };
        audit
            .record_identity(&identity, &request, "attempt", None)
            .await?;
        Ok(Self {
            audit,
            identity,
            request,
        })
    }

    pub(crate) async fn finish(self, succeeded: bool) -> Result<()> {
        self.audit
            .record_identity(
                &self.identity,
                &self.request,
                "outcome",
                Some(if succeeded {
                    StatusCode::OK
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                }),
            )
            .await
    }
}

fn cli_audit_identity(
    cli_actor: Option<String>,
    cli_tenant: Option<String>,
    api_subject: Option<String>,
    api_tenants: Option<String>,
) -> Result<AuditIdentity> {
    let subject = cli_actor
        .filter(|value| !value.trim().is_empty())
        .or_else(|| api_subject.filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| "local-cli".to_string());
    if subject.len() > 128 || subject.chars().any(char::is_control) {
        bail!("BONHOMME_CLI_ACTOR must be at most 128 non-control characters");
    }
    let tenant = match cli_tenant.filter(|value| !value.trim().is_empty()) {
        Some(tenant) => {
            let tenant = tenant.trim().to_string();
            if !valid_tenant(&tenant) {
                bail!("BONHOMME_CLI_TENANT contains an invalid tenant");
            }
            tenant
        }
        None => parse_tenant(api_tenants.filter(|value| !value.trim().is_empty()), false)?,
    };
    Ok(AuditIdentity {
        subject,
        tenant,
        role: "operator".to_string(),
    })
}

fn validate_cli_action(action: &str) -> Result<()> {
    if action.is_empty()
        || action.len() > 96
        || !action
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-' || byte == b'/')
    {
        bail!("CLI audit action must be a static lowercase command path");
    }
    Ok(())
}

struct AuditRequest {
    method: String,
    path: String,
    request_id: Option<String>,
}

impl ApiSecurity {
    #[cfg(test)]
    pub(crate) fn local_test() -> Self {
        Self {
            token_digest: None,
            subject: "test-user".to_string(),
            role: ApiRole::Admin,
            tenant: "local".to_string(),
        }
    }

    pub(crate) fn from_environment(config: &ServerConfig, addr: SocketAddr) -> Result<Self> {
        if config.max_request_bytes == 0 {
            bail!("server.max_request_bytes must be greater than zero");
        }
        if config.request_timeout_seconds == 0 {
            bail!("server.request_timeout_seconds must be greater than zero");
        }
        if config.max_concurrent_requests == 0 {
            bail!("server.max_concurrent_requests must be greater than zero");
        }

        let token = std::env::var("BONHOMME_API_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        let authentication_required = config.require_auth || !addr.ip().is_loopback();
        ensure_credentials(authentication_required, token.as_deref())?;

        let tenant = parse_tenant(
            std::env::var("BONHOMME_API_TENANTS").ok(),
            authentication_required,
        )?;

        Ok(Self {
            token_digest: token.as_deref().map(token_digest),
            subject: std::env::var("BONHOMME_API_SUBJECT")
                .unwrap_or_else(|_| "local-operator".to_string()),
            role: ApiRole::from_env(std::env::var("BONHOMME_API_ROLE").ok())?,
            tenant,
        })
    }

    fn authenticate(&self, headers: &HeaderMap, method: &Method) -> Result<Principal, StatusCode> {
        if let Some(expected) = self.token_digest {
            let presented = headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .ok_or(StatusCode::UNAUTHORIZED)?;
            if !constant_time_eq(&expected, &token_digest(presented)) {
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
        if !self.role.allows(method) {
            return Err(StatusCode::FORBIDDEN);
        }

        if let Some(requested_tenant) = headers.get(TENANT_HEADER) {
            let requested_tenant = requested_tenant
                .to_str()
                .map_err(|_| StatusCode::FORBIDDEN)?;
            if requested_tenant != self.tenant {
                return Err(StatusCode::FORBIDDEN);
            }
        }

        Ok(Principal {
            subject: self.subject.clone(),
            tenant: self.tenant.clone(),
            role: self.role,
        })
    }
}

pub(crate) async fn require_principal(
    State(control): State<ApiControl>,
    mut request: Request,
    next: Next,
) -> Response {
    match control
        .security
        .authenticate(request.headers(), request.method())
    {
        Ok(principal) => {
            let audited = !matches!(
                request.method(),
                &Method::GET | &Method::HEAD | &Method::OPTIONS
            );
            let audit_request = AuditRequest {
                method: request.method().to_string(),
                path: request.uri().path().to_string(),
                request_id: request
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned),
            };
            if audited
                && let Err(error) = control
                    .audit
                    .record(&principal, &audit_request, "attempt", None)
                    .await
            {
                tracing::error!(%error, "failed to persist mutation audit intent");
                return audit_failure_response(true);
            }
            let audit_principal = principal.clone();
            request.extensions_mut().insert(principal);
            let response = next.run(request).await;
            if audited
                && let Err(error) = control
                    .audit
                    .record(
                        &audit_principal,
                        &audit_request,
                        "outcome",
                        Some(response.status()),
                    )
                    .await
            {
                tracing::error!(%error, "failed to persist mutation audit outcome");
                return audit_failure_response(false);
            }
            response
        }
        Err(status) => (
            status,
            axum::Json(serde_json::json!({
                "error": if status == StatusCode::UNAUTHORIZED {
                    "valid bearer authentication is required"
                } else {
                    "the authenticated principal is not authorized for this request"
                }
            })),
        )
            .into_response(),
    }
}

fn audit_failure_response(before_dispatch: bool) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({
            "error": if before_dispatch {
                "the mutation was not dispatched because its durable audit intent could not be written"
            } else {
                "the mutation completed but its durable audit outcome could not be written; inspect server state before retrying"
            }
        })),
    )
        .into_response()
}

#[derive(Clone, Default)]
pub(crate) struct ApiMetrics(Arc<ApiMetricsInner>);

#[derive(Default)]
struct ApiMetricsInner {
    requests: AtomicU64,
    responses: AtomicU64,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
    client_errors: AtomicU64,
    server_errors: AtomicU64,
    total_latency_micros: AtomicU64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApiMetricsSnapshot {
    requests: u64,
    responses: u64,
    in_flight: u64,
    max_in_flight: u64,
    client_errors: u64,
    server_errors: u64,
    total_latency_micros: u64,
}

impl ApiMetrics {
    pub(crate) fn snapshot(&self) -> ApiMetricsSnapshot {
        ApiMetricsSnapshot {
            requests: self.0.requests.load(Ordering::Relaxed),
            responses: self.0.responses.load(Ordering::Relaxed),
            in_flight: self.0.in_flight.load(Ordering::Relaxed),
            max_in_flight: self.0.max_in_flight.load(Ordering::Relaxed),
            client_errors: self.0.client_errors.load(Ordering::Relaxed),
            server_errors: self.0.server_errors.load(Ordering::Relaxed),
            total_latency_micros: self.0.total_latency_micros.load(Ordering::Relaxed),
        }
    }
}

pub(crate) async fn observe_requests(
    State(metrics): State<ApiMetrics>,
    request: Request,
    next: Next,
) -> Response {
    metrics.0.requests.fetch_add(1, Ordering::Relaxed);
    let in_flight = metrics.0.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
    metrics
        .0
        .max_in_flight
        .fetch_max(in_flight, Ordering::Relaxed);
    let started = Instant::now();

    let response = next.run(request).await;

    metrics.0.in_flight.fetch_sub(1, Ordering::Relaxed);
    metrics.0.responses.fetch_add(1, Ordering::Relaxed);
    metrics.0.total_latency_micros.fetch_add(
        started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
        Ordering::Relaxed,
    );
    if response.status().is_client_error() {
        metrics.0.client_errors.fetch_add(1, Ordering::Relaxed);
    } else if response.status().is_server_error() {
        metrics.0.server_errors.fetch_add(1, Ordering::Relaxed);
    }
    response
}

fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn valid_tenant(tenant: &str) -> bool {
    !tenant.is_empty()
        && tenant.len() <= 64
        && tenant
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn parse_tenant(value: Option<String>, authentication_required: bool) -> Result<String> {
    let Some(value) = value else {
        if authentication_required {
            bail!(
                "BONHOMME_API_TENANTS is required when authentication is enabled and must be an explicit allowlist"
            );
        }
        return Ok("local".to_string());
    };
    let tenants = value
        .split(',')
        .map(str::trim)
        .filter(|tenant| !tenant.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if tenants.len() != 1 || tenants[0] == "*" {
        bail!("BONHOMME_API_TENANTS must name exactly one non-wildcard tenant per server process");
    }
    if let Some(invalid) = tenants.iter().find(|tenant| !valid_tenant(tenant)) {
        bail!("BONHOMME_API_TENANTS contains invalid tenant {invalid}");
    }
    Ok(tenants[0].clone())
}

fn ensure_credentials(authentication_required: bool, token: Option<&str>) -> Result<()> {
    if authentication_required {
        let token = token.context(
            "BONHOMME_API_TOKEN is required when server authentication is enabled or the API binds outside loopback",
        )?;
        if token.len() < 32 {
            bail!(
                "BONHOMME_API_TOKEN must contain at least 32 bytes when authentication is required"
            );
        }
    }
    Ok(())
}

fn audit_hash(payload: &AuditPayload) -> Result<String> {
    let digest = Sha256::digest(serde_json::to_vec(payload)?);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn verify_audit_chain(contents: &str) -> Result<(u64, String)> {
    let mut sequence = 0_u64;
    let mut previous_hash = "GENESIS".to_string();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: AuditEntry = serde_json::from_str(line).map_err(|error| {
            anyhow::anyhow!("invalid audit entry on line {}: {error}", index + 1)
        })?;
        if entry.payload.sequence != sequence + 1
            || entry.payload.previous_hash != previous_hash
            || audit_hash(&entry.payload)? != entry.hash
        {
            bail!("audit hash chain verification failed on line {}", index + 1);
        }
        sequence = entry.payload.sequence;
        previous_hash = entry.hash;
    }
    Ok((sequence, previous_hash))
}

/// Verify an audit file without opening it for append. This narrow read-only wrapper is exposed to
/// the production operations command; the chain parser remains private to this module.
pub(crate) async fn verify_audit_file(path: &Path) -> Result<(u64, String)> {
    // Verification stays read-only. New writers create the sidecar lock before the audit file; an
    // older copied audit without a sidecar is safe to verify without creating local state.
    let _lock = acquire_existing_shared_audit_lock(&path.with_extension("jsonl.lock")).await?;
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("reading audit log {}", path.display()))?;
    verify_audit_chain(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn secured(role: ApiRole) -> ApiSecurity {
        ApiSecurity {
            token_digest: Some(token_digest("correct-horse")),
            subject: "test-user".into(),
            role,
            tenant: "acme".into(),
        }
    }

    #[test]
    fn bearer_token_tenant_and_role_are_enforced() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer correct-horse"),
        );
        headers.insert(TENANT_HEADER, HeaderValue::from_static("acme"));

        let principal = secured(ApiRole::Writer)
            .authenticate(&headers, &Method::POST)
            .unwrap();
        assert_eq!(principal.subject, "test-user");
        assert_eq!(principal.tenant, "acme");

        assert_eq!(
            secured(ApiRole::Reader)
                .authenticate(&headers, &Method::POST)
                .unwrap_err(),
            StatusCode::FORBIDDEN
        );
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer wrong"));
        assert_eq!(
            secured(ApiRole::Admin)
                .authenticate(&headers, &Method::GET)
                .unwrap_err(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn invalid_or_ungranted_tenants_are_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer correct-horse"),
        );
        headers.insert(TENANT_HEADER, HeaderValue::from_static("other"));
        assert_eq!(
            secured(ApiRole::Admin)
                .authenticate(&headers, &Method::GET)
                .unwrap_err(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn required_authentication_fails_closed_without_credentials() {
        assert!(ensure_credentials(true, None).is_err());
        assert!(ensure_credentials(true, Some("short")).is_err());
        assert!(ensure_credentials(true, Some(&"x".repeat(32))).is_ok());
        assert!(ensure_credentials(false, None).is_ok());
    }

    #[test]
    fn tenant_allowlists_fail_closed_for_authenticated_deployments() {
        assert_eq!(parse_tenant(None, false).unwrap(), "local");
        assert!(parse_tenant(None, true).is_err());
        assert!(parse_tenant(Some("*".into()), true).is_err());
        assert!(parse_tenant(Some("bad/tenant".into()), true).is_err());
        assert!(parse_tenant(Some("alpha,beta".into()), true).is_err());
        assert_eq!(parse_tenant(Some("alpha".into()), true).unwrap(), "alpha");
    }

    #[tokio::test]
    async fn audit_log_is_durable_and_hash_chained() {
        let directory = std::env::temp_dir().join(format!(
            "bonhomme-audit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("audit.jsonl");
        let audit = AuditLog::open(&path).await.unwrap();
        let principal = Principal {
            subject: "test-user".into(),
            tenant: "acme".into(),
            role: ApiRole::Writer,
        };
        let request = AuditRequest {
            method: "POST".into(),
            path: "/api/demo/reset".into(),
            request_id: Some("request-1".into()),
        };
        audit
            .record(&principal, &request, "attempt", None)
            .await
            .unwrap();
        audit
            .record(&principal, &request, "outcome", Some(StatusCode::OK))
            .await
            .unwrap();
        drop(audit);

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let (sequence, hash) = verify_audit_chain(&contents).unwrap();
        assert_eq!(sequence, 2);
        assert_ne!(hash, "GENESIS");
        assert_eq!(
            verify_audit_file(&path).await.unwrap(),
            (sequence, hash.clone())
        );
        AuditLog::open(&path).await.unwrap();

        std::fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn cli_audit_identity_prefers_explicit_values_and_reuses_api_identity() {
        let explicit = cli_audit_identity(
            Some("developer-cli".into()),
            Some("team-a".into()),
            Some("api-service".into()),
            Some("team-b".into()),
        )
        .unwrap();
        assert_eq!(explicit.subject, "developer-cli");
        assert_eq!(explicit.tenant, "team-a");
        assert_eq!(explicit.role, "operator");

        let inherited = cli_audit_identity(
            None,
            None,
            Some("api-service".into()),
            Some("team-b".into()),
        )
        .unwrap();
        assert_eq!(inherited.subject, "api-service");
        assert_eq!(inherited.tenant, "team-b");
        let local = cli_audit_identity(None, None, None, Some(String::new())).unwrap();
        assert_eq!(local.subject, "local-cli");
        assert_eq!(local.tenant, "local");
        assert!(cli_audit_identity(None, None, None, Some("team-a,team-b".into())).is_err());
    }

    #[tokio::test]
    async fn cli_mutation_audit_records_static_metadata_and_a_durable_outcome() {
        let directory = std::env::temp_dir().join(format!(
            "bonhomme-cli-audit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let audit = CliMutationAudit::begin_with_identity(
            &directory,
            "slice/apply",
            AuditIdentity {
                subject: "developer-cli".into(),
                tenant: "team-a".into(),
                role: "operator".into(),
            },
        )
        .await
        .unwrap();
        audit.finish(true).await.unwrap();

        let path = directory.join(".bonhomme").join("audit.jsonl");
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let entries = contents
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["payload"]["method"], "CLI");
        assert_eq!(entries[0]["payload"]["path"], "/cli/slice/apply");
        assert_eq!(entries[0]["payload"]["subject"], "developer-cli");
        assert_eq!(entries[0]["payload"]["tenant"], "team-a");
        assert_eq!(entries[0]["payload"]["role"], "operator");
        assert_eq!(entries[0]["payload"]["phase"], "attempt");
        assert_eq!(entries[1]["payload"]["phase"], "outcome");
        assert_eq!(entries[1]["payload"]["status"], 200);
        assert_eq!(
            entries[0]["payload"]["requestId"],
            entries[1]["payload"]["requestId"]
        );
        assert!(!contents.contains("databaseUrl"));
        assert!(!contents.contains("arguments"));
        assert_eq!(verify_audit_file(&path).await.unwrap().0, 2);

        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn independent_audit_writers_refresh_the_chain_under_a_file_lock() {
        let directory = std::env::temp_dir().join(format!(
            "bonhomme-concurrent-audit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("audit.jsonl");
        let first = AuditLog::open(&path).await.unwrap();
        let second = AuditLog::open(&path).await.unwrap();
        let principal = Principal {
            subject: "operator".into(),
            tenant: "team-a".into(),
            role: ApiRole::Admin,
        };
        let request = AuditRequest {
            method: "CLI".into(),
            path: "/cli/init".into(),
            request_id: Some("cli-concurrency-test".into()),
        };

        first
            .record(&principal, &request, "attempt", None)
            .await
            .unwrap();
        second
            .record(&principal, &request, "outcome", Some(StatusCode::OK))
            .await
            .unwrap();
        first
            .record(&principal, &request, "attempt", None)
            .await
            .unwrap();

        assert_eq!(verify_audit_file(&path).await.unwrap().0, 3);
        drop(first);
        drop(second);
        std::fs::remove_dir_all(directory).ok();
    }

    #[tokio::test]
    async fn audit_verification_does_not_create_a_lock_for_a_copied_log() {
        let directory = std::env::temp_dir().join(format!(
            "bonhomme-readonly-audit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&directory).await.unwrap();
        let path = directory.join("audit.jsonl");
        tokio::fs::write(&path, "").await.unwrap();

        assert_eq!(
            verify_audit_file(&path).await.unwrap(),
            (0, "GENESIS".to_string())
        );
        assert!(!path.with_extension("jsonl.lock").exists());
        std::fs::remove_dir_all(directory).ok();
    }
}
