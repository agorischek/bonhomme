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
    /// Per-language toolchain binary paths. `typescript`/`tsc`, `python`/`python3`, `dotnet`, and
    /// `elixirc`/`elixir` are wired at the composition root; other plugins can opt in without
    /// changing the schema.
    pub toolchain: BTreeMap<String, String>,
    /// Per-extension formatter commands applied at the write/compare boundary (see `crate::format`).
    /// Keyed by file extension; the command reads content on stdin and writes the formatted result
    /// to stdout, e.g. `rs = "rustfmt"`, `ts = "prettier --stdin-filepath {path}"`.
    pub format: BTreeMap<String, String>,
    /// Git integration mode — gates write-back into the working tree (`bonhomme session land`).
    pub git: GitConfig,
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

/// Walk up from `start` to find the project root and load its config. The root is the nearest
/// ancestor containing `bonhomme.toml` (which is parsed), else the nearest containing `.git`, else
/// `start` itself. The root anchors the default database location.
pub fn discover(start: &Path) -> Result<(Config, PathBuf)> {
    let mut git_root: Option<PathBuf> = None;
    for dir in start.ancestors() {
        let manifest = dir.join("bonhomme.toml");
        if manifest.is_file() {
            let text = std::fs::read_to_string(&manifest)
                .with_context(|| format!("reading {}", manifest.display()))?;
            let config =
                toml::from_str(&text).with_context(|| format!("parsing {}", manifest.display()))?;
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
    fn parses_toolchain_section() {
        let config: Config = toml::from_str("[toolchain]\ntypescript = \"tsgo\"\n").unwrap();
        assert_eq!(
            config.toolchain.get("typescript").map(String::as_str),
            Some("tsgo")
        );
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
