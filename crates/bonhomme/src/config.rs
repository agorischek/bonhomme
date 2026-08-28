//! Project configuration — the "config spine". Parsed once here at the composition root and used to
//! resolve the values the rest of the system already takes by injection. Config is OPTIONAL: with no
//! `bonhomme.toml` present, the defaults reproduce a zero-infra local setup — an embedded Turso
//! database under `.bonhomme/`, no database server required.
//! See `reports/config-plan.md`.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The on-disk `bonhomme.toml` schema. Every section is optional; unknown keys are rejected so a
/// typo fails loudly rather than being silently ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub storage: StorageConfig,
    /// HTTP server security, resource, and timeout policy. Authentication secrets are read only
    /// from environment variables and are never accepted in this file.
    pub server: ServerConfig,
    /// Per-language toolchain binary paths. `typescript`/`tsc`, `python`/`python3`, `dotnet`,
    /// `java`/`javac`, `php`, `ruby`, `kotlin`/`kotlinc`, `swift`/`swiftc`, and
    /// `elixirc`/`elixir` are wired at the composition root; other plugins can opt in without
    /// changing the schema.
    pub toolchain: BTreeMap<String, String>,
    /// Per-extension formatter commands applied at the write/compare boundary (see `crate::format`).
    /// Keyed by file extension; the command reads content on stdin and writes the formatted result
    /// to stdout, e.g. `rs = "rustfmt"`, `ts = "prettier --stdin-filepath {path}"`.
    pub format: BTreeMap<String, String>,
    /// Git integration mode — gates write-back into the working tree (`bonhomme session land`).
    pub git: GitConfig,
    /// Validation policy for workflow entry points. Heavy language toolchain checks are useful, but
    /// should be opt-in for session startup because real repos may need dependencies, services, or
    /// generated artifacts before they build.
    pub validation: ValidationConfig,
    /// Audit durability and export policy. Local development defaults to a durable, hash-chained
    /// file; production must explicitly require export into an external immutable system.
    pub audit: AuditConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Require bearer authentication even when listening on loopback. Non-loopback listeners
    /// always require credentials regardless of this setting.
    pub require_auth: bool,
    /// Maximum request body accepted by the API.
    pub max_request_bytes: usize,
    /// Maximum duration for one HTTP request.
    pub request_timeout_seconds: u64,
    /// Maximum number of requests executing concurrently in one server process.
    pub max_concurrent_requests: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            require_auth: false,
            max_request_bytes: 2 * 1024 * 1024,
            request_timeout_seconds: 120,
            max_concurrent_requests: 64,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// `postgres://…` for the hosted server, or `turso:`/`sqlite:`/`file:`/`:memory:` for an
    /// embedded database. Absent → the computed project-local Turso default.
    pub database_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitConfig {
    pub write_back: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ValidationConfig {
    pub session_start: SessionValidationMode,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditConfig {
    pub export_policy: AuditExportPolicy,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum AuditExportPolicy {
    /// Durable and tamper-evident on the local host, but not protected from host-level deletion.
    #[default]
    LocalOnly,
    /// Deployment automation exports and anchors audit heads in an external WORM/immutable system.
    ExternalWorm,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SessionValidationMode {
    /// Check semantic graph invariants only; skip language toolchain build/typecheck gates.
    #[default]
    None,
    /// Also run each language plugin's toolchain validator over the rendered projection.
    Toolchain,
}

impl SessionValidationMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Toolchain => "toolchain",
        }
    }

    pub fn toolchain_enabled(self) -> bool {
        matches!(self, Self::Toolchain)
    }
}

/// Walk up from `start` to find the project root and load its config. The root is the nearest
/// ancestor containing `bonhomme.toml` (which is parsed), else the nearest containing `.git`, else
/// `start` itself. The root anchors the default database location.
pub fn discover(start: &Path) -> Result<(Config, PathBuf)> {
    let mut git_root: Option<PathBuf> = None;
    for dir in start.ancestors() {
        let manifest = dir.join("bonhomme.toml");
        if manifest.is_file() {
            let config = load(&manifest)?;
            return Ok((config, dir.to_path_buf()));
        }
        if git_root.is_none() && dir.join(".git").exists() {
            git_root = Some(dir.to_path_buf());
        }
    }
    Ok((
        Config::default(),
        git_root.unwrap_or_else(|| start.to_path_buf()),
    ))
}

/// Load one explicit manifest. Used by deployment checks so a checked-in production template can
/// be validated without copying it over the developer's local configuration.
pub fn load(path: &Path) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Resolve the storage URL with precedence: CLI flag / `DATABASE_URL` env (collapsed by clap into
/// `flag_or_env`) > `bonhomme.toml` > the computed project-local Turso default.
pub fn resolve_database_url(flag_or_env: Option<String>, config: &Config, root: &Path) -> String {
    flag_or_env
        .or_else(|| config.storage.database_url.clone())
        .unwrap_or_else(|| default_database_url(root))
}

/// The zero-config default: an embedded Turso database under `<root>/.bonhomme/bonhomme.db`, so a
/// fresh checkout needs no database server at all.
fn default_database_url(root: &Path) -> String {
    let path = root.join(".bonhomme").join("bonhomme.db");
    format!("turso:{}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_url(url: &str) -> Config {
        Config {
            storage: StorageConfig {
                database_url: Some(url.into()),
            },
            ..Config::default()
        }
    }

    #[test]
    fn flag_or_env_outranks_file() {
        let config = config_with_url("postgres://from-file");
        assert_eq!(
            resolve_database_url(
                Some("postgres://from-flag".into()),
                &config,
                Path::new("/repo")
            ),
            "postgres://from-flag"
        );
    }

    #[test]
    fn file_outranks_default() {
        let config = config_with_url("turso:/data/x.db");
        assert_eq!(
            resolve_database_url(None, &config, Path::new("/repo")),
            "turso:/data/x.db"
        );
    }

    #[test]
    fn default_is_project_local_turso() {
        assert_eq!(
            resolve_database_url(None, &Config::default(), Path::new("/repo")),
            "turso:/repo/.bonhomme/bonhomme.db"
        );
    }

    #[test]
    fn parses_storage_section() {
        let config: Config =
            toml::from_str("[storage]\ndatabase_url = \"turso:./x.db\"\n").unwrap();
        assert_eq!(config.storage.database_url.as_deref(), Some("turso:./x.db"));
    }

    #[test]
    fn parses_server_resource_policy() {
        let config: Config = toml::from_str(
            "[server]\nrequire_auth = true\nmax_request_bytes = 4096\nrequest_timeout_seconds = 30\nmax_concurrent_requests = 8\n",
        )
        .unwrap();
        assert!(config.server.require_auth);
        assert_eq!(config.server.max_request_bytes, 4096);
        assert_eq!(config.server.request_timeout_seconds, 30);
        assert_eq!(config.server.max_concurrent_requests, 8);
    }

    #[test]
    fn parses_toolchain_section() {
        let config: Config = toml::from_str("[toolchain]\ntypescript = \"tsgo\"\n").unwrap();
        assert_eq!(
            config.toolchain.get("typescript").map(String::as_str),
            Some("tsgo")
        );
    }

    #[test]
    fn session_start_validation_defaults_to_none() {
        let config = Config::default();
        assert_eq!(config.validation.session_start, SessionValidationMode::None);
    }

    #[test]
    fn parses_validation_section() {
        let config: Config =
            toml::from_str("[validation]\nsession_start = \"toolchain\"\n").unwrap();
        assert_eq!(
            config.validation.session_start,
            SessionValidationMode::Toolchain
        );
    }

    #[test]
    fn audit_export_policy_defaults_local_and_parses_external_worm() {
        assert_eq!(
            Config::default().audit.export_policy,
            AuditExportPolicy::LocalOnly
        );
        let config: Config =
            toml::from_str("[audit]\nexport_policy = \"external-worm\"\n").unwrap();
        assert_eq!(config.audit.export_policy, AuditExportPolicy::ExternalWorm);
    }

    #[test]
    fn rejects_unknown_key() {
        let err = toml::from_str::<Config>("bogus = true\n").unwrap_err();
        assert!(err.to_string().contains("bogus") || err.to_string().contains("unknown"));
    }

    #[test]
    fn discover_reads_nearest_manifest() {
        let base = std::env::temp_dir().join(format!("bonhomme-cfg-{}", std::process::id()));
        let sub = base.join("proj").join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            base.join("proj").join("bonhomme.toml"),
            "[storage]\ndatabase_url = \"turso:/custom/db\"\n",
        )
        .unwrap();

        let (config, root) = discover(&sub).unwrap();
        assert_eq!(
            config.storage.database_url.as_deref(),
            Some("turso:/custom/db")
        );
        assert_eq!(root, base.join("proj"));

        std::fs::remove_dir_all(&base).ok();
    }
}
