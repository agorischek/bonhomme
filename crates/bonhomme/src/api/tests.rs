use super::{
    ApiError, RealSwarmLaunchGuard, SWARM_API_ENV, SWARM_ENV_ALLOWLIST, api_base_url,
    api_conflict_workspace_directory, node_probe_environment, probe_node, public_swarm_log_path,
    router, secure_private_directory, secure_private_file, swarm_coordinator_environment,
    validate_api_conflict_workspace,
};
use crate::demo::RealSwarmStatus;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::IntoResponse,
};
use bonhomme_engine::Storage;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use tower::ServiceExt;

struct RuntimeTestApp {
    router: Router,
    root: PathBuf,
}

impl Drop for RuntimeTestApp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).ok();
    }
}

async fn runtime_test_app(config: crate::config::Config) -> RuntimeTestApp {
    let root = std::env::temp_dir().join(format!(
        "bonhomme-runtime-api-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let storage = Storage::connect(
        ":memory:",
        crate::plugins::language_registry(&config, &root),
    )
    .await
    .unwrap();
    storage.migrate().await.unwrap();
    let control =
        crate::security::ApiControl::new(crate::security::ApiSecurity::local_test(), &root)
            .await
            .unwrap();
    RuntimeTestApp {
        router: router(
            storage,
            root.clone(),
            "http://127.0.0.1:3030".to_string(),
            control,
            &config,
        ),
        root,
    }
}

#[test]
fn api_base_url_formats_ipv4_and_ipv6_addresses() {
    assert_eq!(
        api_base_url("127.0.0.1:3030".parse().unwrap()),
        "http://127.0.0.1:3030"
    );
    assert_eq!(
        api_base_url("[::1]:3030".parse().unwrap()),
        "http://[::1]:3030"
    );
    assert_eq!(
        api_base_url("[::]:3030".parse().unwrap()),
        "http://127.0.0.1:3030"
    );
}

#[test]
fn swarm_log_paths_exposed_by_the_api_are_repository_relative() {
    let root = std::path::Path::new("/srv/bonhomme");
    let log = root.join(".bonhomme/web-swarm/run-1/runner.log");
    assert_eq!(
        public_swarm_log_path(root, &log).unwrap(),
        ".bonhomme/web-swarm/run-1/runner.log"
    );
    assert!(public_swarm_log_path(root, std::path::Path::new("/tmp/runner.log")).is_err());
}

#[cfg(unix)]
#[test]
fn swarm_directories_and_logs_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("bonhomme-log-mode-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let log = root.join("runner.log");
    std::fs::write(&log, "log").unwrap();
    secure_private_directory(&root).unwrap();
    secure_private_file(&log).unwrap();
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn missing_real_swarm_script_is_a_server_error() {
    let response = ApiError(anyhow::anyhow!(
        "real swarm script is unavailable at demo/scripts/codex-swarm.mjs"
    ))
    .into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn internal_api_errors_do_not_expose_local_diagnostics() {
    let response = ApiError(anyhow::anyhow!(
        "deployment database failed with internal driver detail"
    ))
    .into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert_eq!(body, r#"{"error":"internal server error"}"#);
}

#[test]
fn conflict_workspace_client_errors_have_stable_4xx_statuses() {
    let traversal = ApiError(anyhow::anyhow!(
        "invalid conflict workspace directory: path traversal"
    ))
    .into_response();
    assert_eq!(traversal.status(), StatusCode::BAD_REQUEST);

    let mismatched = ApiError(anyhow::anyhow!(
        "checked-out conflict id one does not match two"
    ))
    .into_response();
    assert_eq!(mismatched.status(), StatusCode::CONFLICT);
}

#[test]
fn swarm_agent_environment_does_not_inherit_service_credentials() {
    for forbidden in [
        "BONHOMME_API_TOKEN",
        "BONHOMME_API_TENANTS",
        "DATABASE_URL",
        "PGPASSWORD",
    ] {
        assert!(!SWARM_ENV_ALLOWLIST.contains(&forbidden));
    }
    assert_eq!(SWARM_API_ENV, &["BONHOMME_API_TOKEN"]);

    let selected = swarm_coordinator_environment([
        ("PATH".into(), "/bin".into()),
        ("LC_CTYPE".into(), "en_US.UTF-8".into()),
        ("BONHOMME_API_TOKEN".into(), "coordinator-token".into()),
        ("BONHOMME_API_TENANTS".into(), "tenant-a".into()),
        ("DATABASE_URL".into(), "postgres://service".into()),
        ("PGPASSWORD".into(), "database-password".into()),
    ]);
    let selected = selected
        .into_iter()
        .map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap()))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(selected.get("PATH").map(String::as_str), Some("/bin"));
    assert_eq!(
        selected.get("BONHOMME_API_TOKEN").map(String::as_str),
        Some("coordinator-token")
    );
    assert_eq!(
        selected.get("LC_CTYPE").map(String::as_str),
        Some("en_US.UTF-8")
    );
    assert!(!selected.contains_key("BONHOMME_API_TENANTS"));
    assert!(!selected.contains_key("DATABASE_URL"));
    assert!(!selected.contains_key("PGPASSWORD"));
}

#[cfg(unix)]
#[tokio::test]
async fn node_preflight_clears_service_credentials_and_node_options() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("bonhomme-node-probe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let fake_node = root.join("node");
    std::fs::write(
        &fake_node,
        "#!/bin/sh\n[ -z \"$BONHOMME_API_TOKEN$DATABASE_URL$PGPASSWORD$NODE_OPTIONS\" ]\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&fake_node).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&fake_node, permissions).unwrap();

    let environment = node_probe_environment([
        ("PATH".into(), "/bin".into()),
        ("BONHOMME_API_TOKEN".into(), "service-token".into()),
        ("DATABASE_URL".into(), "postgres://service".into()),
        ("PGPASSWORD".into(), "database-password".into()),
        (
            "NODE_OPTIONS".into(),
            "--require=/tmp/steal-secrets.js".into(),
        ),
    ]);
    assert!(
        probe_node(fake_node.as_os_str(), environment)
            .await
            .unwrap()
            .success()
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn api_conflict_workspace_is_bound_to_the_requested_conflict() {
    let id = uuid::Uuid::new_v4();
    let expected = std::path::PathBuf::from(".bonhomme")
        .join("conflicts")
        .join(id.to_string());
    assert_eq!(
        api_conflict_workspace_directory(id, None).unwrap(),
        expected
    );
    assert_eq!(
        api_conflict_workspace_directory(id, Some(&expected.join("resolved"))).unwrap(),
        expected.join("resolved")
    );

    for unsafe_path in [
        std::path::Path::new("../outside"),
        std::path::Path::new("/tmp/outside"),
        std::path::Path::new(".bonhomme/conflicts/someone-else"),
    ] {
        assert!(api_conflict_workspace_directory(id, Some(unsafe_path)).is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn api_conflict_workspace_rejects_symlinked_ancestors() {
    use std::os::unix::fs::symlink;

    let id = uuid::Uuid::new_v4();
    let root = std::env::temp_dir().join(format!("bonhomme-api-conflict-root-{id}"));
    let outside = std::env::temp_dir().join(format!("bonhomme-api-conflict-outside-{id}"));
    std::fs::create_dir_all(
        outside
            .join("conflicts")
            .join(id.to_string())
            .join("resolved"),
    )
    .unwrap();
    std::fs::create_dir_all(&root).unwrap();
    symlink(&outside, root.join(".bonhomme")).unwrap();

    let relative = std::path::PathBuf::from(".bonhomme")
        .join("conflicts")
        .join(id.to_string());
    assert!(
        validate_api_conflict_workspace(&root, id, &relative)
            .await
            .is_err()
    );

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(outside).ok();
}

#[tokio::test]
async fn dropped_real_swarm_launch_guard_clears_running_status() {
    let status = Arc::new(RwLock::new(RealSwarmStatus {
        running: true,
        ..RealSwarmStatus::default()
    }));
    drop(RealSwarmLaunchGuard::new(status.clone()));

    let status = status.read().await;
    assert!(!status.running);
    assert_eq!(
        status.error.as_deref(),
        Some("real swarm launch was cancelled")
    );
}

#[tokio::test]
async fn durable_job_api_derives_tenant_from_the_authenticated_principal() {
    let root = std::env::temp_dir().join(format!(
        "bonhomme-job-api-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let config = crate::config::Config::default();
    let storage = Storage::connect(
        ":memory:",
        crate::plugins::language_registry(&config, &root),
    )
    .await
    .unwrap();
    storage.migrate().await.unwrap();
    let (repository, _) = storage.init_repository("job-api").await.unwrap();
    let control =
        crate::security::ApiControl::new(crate::security::ApiSecurity::local_test(), &root)
            .await
            .unwrap();
    let app = router(
        storage.clone(),
        root.clone(),
        "http://127.0.0.1:3030".to_string(),
        control,
        &config,
    );

    let response = app
        .clone()
        .oneshot(
            Request::post("/api/repos/job-api/jobs")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"semantic-merge","idempotencyKey":"request-1","payload":{"branch":"feature"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let jobs = storage
        .list_agent_jobs("local", repository.id)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].tenant_key, "local");
    assert_eq!(jobs[0].kind, "semantic-merge");

    let collision = app
        .oneshot(
            Request::post("/api/repos/job-api/jobs")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"kind":"semantic-merge","idempotencyKey":"request-1","payload":{"branch":"different"}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        collision.status(),
        StatusCode::CONFLICT,
        "Turso idempotency collisions must be exposed as an HTTP conflict"
    );

    std::fs::remove_dir_all(root).ok();
}

#[tokio::test]
async fn runtime_stack_generates_and_propagates_request_ids() {
    let app = runtime_test_app(crate::config::Config::default()).await;

    let generated = app
        .router
        .clone()
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let generated = generated
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    uuid::Uuid::parse_str(generated).unwrap();

    let supplied = "caller-request-id";
    let propagated = app
        .router
        .clone()
        .oneshot(
            Request::get("/api/health")
                .header("x-request-id", supplied)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        propagated
            .headers()
            .get("x-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        supplied
    );
}

#[tokio::test]
async fn runtime_metrics_observe_requests_responses_and_latency() {
    let app = runtime_test_app(crate::config::Config::default()).await;

    let delayed = app
        .router
        .clone()
        .oneshot(
            Request::get("/__test/runtime/delay/5")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delayed.status(), StatusCode::OK);
    let not_found = app
        .router
        .clone()
        .oneshot(
            Request::get("/__test/runtime/missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_found.status(), StatusCode::NOT_FOUND);

    let metrics = app
        .router
        .clone()
        .oneshot(Request::get("/api/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    let body = to_bytes(metrics.into_body(), 16 * 1024).await.unwrap();
    let metrics: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(metrics["requests"], 3);
    assert_eq!(metrics["responses"], 2);
    assert_eq!(metrics["inFlight"], 1);
    assert_eq!(metrics["maxInFlight"], 1);
    assert_eq!(metrics["clientErrors"], 1);
    assert_eq!(metrics["serverErrors"], 0);
    assert!(metrics["totalLatencyMicros"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn runtime_stack_rejects_request_bodies_over_the_configured_limit() {
    let mut config = crate::config::Config::default();
    config.server.max_request_bytes = 8;
    let app = runtime_test_app(config).await;

    let response = app
        .router
        .clone()
        .oneshot(
            Request::post("/__test/runtime/echo")
                .header("content-type", "text/plain")
                .header("content-length", "9")
                .body(Body::from("123456789"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn runtime_stack_times_out_slow_requests() {
    let mut config = crate::config::Config::default();
    config.server.request_timeout_seconds = 1;
    let app = runtime_test_app(config).await;

    let started = Instant::now();
    let response = app
        .router
        .clone()
        .oneshot(
            Request::get("/__test/runtime/delay/1500")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert!(started.elapsed() < Duration::from_millis(1400));
}

#[tokio::test]
async fn runtime_stack_enforces_the_configured_concurrency_limit() {
    let mut config = crate::config::Config::default();
    config.server.max_concurrent_requests = 1;
    config.server.request_timeout_seconds = 2;
    let app = runtime_test_app(config).await;
    let request = || {
        Request::get("/__test/runtime/delay/120")
            .body(Body::empty())
            .unwrap()
    };

    let started = Instant::now();
    let (first, second) = tokio::join!(
        app.router.clone().oneshot(request()),
        app.router.clone().oneshot(request())
    );
    assert_eq!(first.unwrap().status(), StatusCode::OK);
    assert_eq!(second.unwrap().status(), StatusCode::OK);
    assert!(started.elapsed() >= Duration::from_millis(200));
}
