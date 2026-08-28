//! Executable production-readiness gates. These checks intentionally do not contact the database
//! or mutate server state, which makes the same command suitable for CI, deployment preflight, and
//! incident-response verification.

use crate::config::Config;
use anyhow::{Result, bail};
use serde::Serialize;
use std::{net::SocketAddr, path::Path};

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Passed,
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductionCheck {
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductionReport {
    schema_version: u32,
    ready: bool,
    checks: Vec<ProductionCheck>,
}

pub(crate) fn check_production(
    config: &Config,
    database_url: &str,
    addr: SocketAddr,
) -> Result<()> {
    let token = std::env::var("BONHOMME_API_TOKEN").unwrap_or_default();
    let tenants = std::env::var("BONHOMME_API_TENANTS").unwrap_or_default();
    let subject = std::env::var("BONHOMME_API_SUBJECT").unwrap_or_default();
    let role = std::env::var("BONHOMME_API_ROLE").unwrap_or_default();
    let mut checks = Vec::new();

    push_check(
        &mut checks,
        "postgres_storage",
        database_url.starts_with("postgres://") || database_url.starts_with("postgresql://"),
        format!(
            "resolved storage is {}",
            if database_url.starts_with("postgres") {
                "Postgres"
            } else {
                "embedded/local"
            }
        ),
    );
    push_check(
        &mut checks,
        "storage_secret_not_in_manifest",
        config.storage.database_url.is_none(),
        if config.storage.database_url.is_none() {
            "DATABASE_URL is supplied at deployment time".to_string()
        } else {
            "remove storage.database_url from the production manifest and inject DATABASE_URL"
                .to_string()
        },
    );
    push_check(
        &mut checks,
        "authentication_required",
        config.server.require_auth,
        "server.require_auth must be true even behind a private ingress".to_string(),
    );
    push_check(
        &mut checks,
        "api_token",
        token.len() >= 32 && !looks_like_placeholder(&token),
        "BONHOMME_API_TOKEN must be a non-placeholder secret of at least 32 bytes".to_string(),
    );

    let tenant_values = tenants
        .split(',')
        .map(str::trim)
        .filter(|tenant| !tenant.is_empty())
        .collect::<Vec<_>>();
    push_check(
        &mut checks,
        "tenant_allowlist",
        tenant_values.len() == 1 && !tenant_values.contains(&"*"),
        "BONHOMME_API_TENANTS must name exactly one non-wildcard tenant per server process"
            .to_string(),
    );
    push_check(
        &mut checks,
        "principal_subject",
        !subject.trim().is_empty()
            && subject != "local-operator"
            && !looks_like_placeholder(&subject),
        "BONHOMME_API_SUBJECT must identify the deployed workload or operator".to_string(),
    );
    push_check(
        &mut checks,
        "principal_role",
        matches!(role.as_str(), "reader" | "writer" | "admin"),
        "BONHOMME_API_ROLE must explicitly be reader, writer, or admin".to_string(),
    );
    push_check(
        &mut checks,
        "request_body_limit",
        (1..=16 * 1024 * 1024).contains(&config.server.max_request_bytes),
        format!(
            "server.max_request_bytes={} (allowed production range: 1..=16777216)",
            config.server.max_request_bytes
        ),
    );
    push_check(
        &mut checks,
        "request_timeout",
        (1..=300).contains(&config.server.request_timeout_seconds),
        format!(
            "server.request_timeout_seconds={} (allowed production range: 1..=300)",
            config.server.request_timeout_seconds
        ),
    );
    push_check(
        &mut checks,
        "concurrency_limit",
        (1..=1024).contains(&config.server.max_concurrent_requests),
        format!(
            "server.max_concurrent_requests={} (allowed production range: 1..=1024)",
            config.server.max_concurrent_requests
        ),
    );
    push_check(
        &mut checks,
        "network_listener",
        !addr.ip().is_unspecified() || config.server.require_auth,
        format!("listener {addr} is protected by mandatory authentication"),
    );
    push_check(
        &mut checks,
        "working_tree_writeback_disabled",
        !config.git.write_back,
        "git.write_back must remain false in the server deployment manifest".to_string(),
    );
    push_check(
        &mut checks,
        "external_audit_anchoring",
        external_audit_anchoring_configured(config),
        concat!(
            "audit.export_policy must be external-worm and deployment automation must ",
            "export/anchor audit heads outside the service trust boundary"
        )
        .to_string(),
    );

    let ready = checks
        .iter()
        .all(|check| matches!(check.status, CheckStatus::Passed));
    let report = ProductionReport {
        schema_version: 1,
        ready,
        checks,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !ready {
        bail!("production configuration check failed");
    }
    Ok(())
}

fn external_audit_anchoring_configured(config: &Config) -> bool {
    config.audit.export_policy == crate::config::AuditExportPolicy::ExternalWorm
}

pub(crate) async fn verify_audit(path: &Path) -> Result<()> {
    let (sequence, head_hash) = crate::security::verify_audit_file(path).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "valid": true,
            "path": path,
            "entries": sequence,
            "headHash": head_hash,
        }))?
    );
    Ok(())
}

fn push_check(checks: &mut Vec<ProductionCheck>, name: &'static str, passed: bool, detail: String) {
    checks.push(ProductionCheck {
        name,
        status: if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        detail,
    });
}

fn looks_like_placeholder(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains('<')
        || value.contains('>')
        || [
            "changeme",
            "replace-me",
            "example",
            "placeholder",
            "test-only",
            "not-a-secret",
            "dummy",
        ]
        .iter()
        .any(|placeholder| value.contains(placeholder))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuditExportPolicy, GitConfig, ServerConfig};

    #[test]
    fn placeholder_detection_is_conservative() {
        assert!(looks_like_placeholder("replace-me-before-deploy"));
        assert!(looks_like_placeholder("EXAMPLE-token"));
        assert!(looks_like_placeholder("ci-test-only-not-a-secret"));
        assert!(looks_like_placeholder("<at-least-32-random-bytes>"));
        assert!(!looks_like_placeholder("4f04f5cc59684d14bc33d585425c310e"));
    }

    #[test]
    fn production_defaults_make_security_intent_explicit() {
        let config = Config::default();
        assert!(!config.server.require_auth);
        assert!(!config.git.write_back);
        assert_eq!(config.audit.export_policy, AuditExportPolicy::LocalOnly);
        assert!(!external_audit_anchoring_configured(&config));
        let hardened = Config {
            server: ServerConfig {
                require_auth: true,
                ..ServerConfig::default()
            },
            git: GitConfig { write_back: false },
            audit: crate::config::AuditConfig {
                export_policy: AuditExportPolicy::ExternalWorm,
            },
            ..Config::default()
        };
        assert!(hardened.server.require_auth);
        assert!(external_audit_anchoring_configured(&hardened));
    }
}
