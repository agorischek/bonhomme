use std::path::Path;

use anyhow::{Context, Result};
use clap::Args;

use crate::{cli::session::ActiveSession, config::Config};

#[derive(Args)]
pub(super) struct AgentsArgs {
    /// Logical bonhomme repository name. Defaults to the active session repo, then repo root name.
    #[arg(long)]
    repo: Option<String>,
    /// Agent branch/name used in example commands.
    #[arg(long, default_value = "agent-001")]
    agent: String,
    /// Base branch used in example commands. Defaults to the active session branch, then main.
    #[arg(long)]
    base: Option<String>,
    /// Symbol to use in the slice example.
    #[arg(long)]
    symbol: Option<String>,
}

pub(super) async fn run(
    args: AgentsArgs,
    config: &Config,
    root: &Path,
    active_session: Option<&ActiveSession>,
    explicit_database_url: Option<&str>,
    resolved_database_url: &str,
) -> Result<()> {
    let repo = args
        .repo
        .or_else(|| active_session.map(|session| session.repository.clone()))
        .map(Ok)
        .unwrap_or_else(|| default_repository_name(root))?;
    let base = args
        .base
        .or_else(|| active_session.map(|session| session.branch.clone()))
        .unwrap_or_else(|| "main".to_string());
    let symbol = args.symbol.unwrap_or_else(|| "<symbol>".to_string());
    let storage_source = storage_source(config, active_session, explicit_database_url);
    let session_status = active_session
        .map(|session| {
            format!(
                "active (repo `{}`, branch `{}`, base op {})",
                session.repository, session.branch, session.base_position
            )
        })
        .unwrap_or_else(|| "not active".to_string());

    println!("# Bonhomme Agent Instructions");
    println!();
    println!("No environment variables are required.");
    println!();
    println!("## Context");
    println!();
    println!("- Repo root: `{}`", root.display());
    println!("- Logical repo: `{repo}`");
    println!("- Base branch: `{base}`");
    println!("- Storage: `{resolved_database_url}` ({storage_source})");
    println!("- Session: {session_status}");
    println!(
        "- Git write-back: {}",
        if config.git.write_back {
            "enabled"
        } else {
            "disabled"
        }
    );
    print_toolchains(config);
    print_formatters(config);
    print_session_validation(config);
    println!();

    println!("## Workflow");
    println!();
    if active_session.is_none() {
        println!("Start a local Bonhomme session before agent work:");
        println!();
        println!("```sh");
        println!("{}", session_start_command(config));
        println!("```");
        println!();
        println!("After that, repo-scoped commands will use the session automatically.");
        println!();
    } else {
        println!("Repo-scoped commands use the active session automatically.");
        println!();
    }

    println!("1. Create your agent branch.");
    println!();
    println!("```sh");
    println!(
        "bonhomme branch create --name {} --base {}",
        shell_word(&args.agent),
        shell_word(&base)
    );
    println!("```");
    println!();

    println!("2. Request an editable slice.");
    println!();
    println!("```sh");
    println!(
        "bonhomme slice create --branch {} --symbol {} > slice.json",
        shell_word(&args.agent),
        shell_word(&symbol)
    );
    println!("```");
    println!();

    println!("3. Edit `slice.json` by changing `files[].content`. Keep the slice `id`.");
    println!();

    println!("4. Apply the edited slice back as semantic operations.");
    println!();
    println!("```sh");
    println!(
        "bonhomme slice apply --modified slice.json --title \"<short task title>\" --agent {}",
        shell_word(&args.agent)
    );
    println!("```");
    println!();

    println!("5. Validate your branch.");
    println!();
    println!("```sh");
    println!("bonhomme validate --branch {}", shell_word(&args.agent));
    println!("```");
    println!();

    println!("6. Merge into the base branch.");
    println!();
    println!("```sh");
    println!(
        "bonhomme merge --source {} --target {}",
        shell_word(&args.agent),
        shell_word(&base)
    );
    println!("```");
    println!();

    println!("## Conflict Policy");
    println!();
    println!(
        "If Bonhomme reports `CONFLICT`, stop and report the semantic conflict. Do not guess a text merge."
    );
    println!();

    println!("## Landing");
    println!();
    if config.git.write_back {
        println!("In-place write-back is enabled for this repo:");
        println!();
        println!("```sh");
        println!("bonhomme session land");
        println!("```");
    } else {
        println!("In-place write-back is disabled for this repo. Land to an output directory:");
        println!();
        println!("```sh");
        println!("bonhomme session land --out rendered-session");
        println!("```");
    }
    println!();
    println!("Review the final Git diff before committing.");
    Ok(())
}

fn default_repository_name(root: &Path) -> Result<String> {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .with_context(|| format!("could not infer repository name from {}", root.display()))
}

fn storage_source(
    config: &Config,
    active_session: Option<&ActiveSession>,
    explicit_database_url: Option<&str>,
) -> &'static str {
    if explicit_database_url.is_some() {
        return "explicit storage override";
    }
    if active_session.is_some() {
        return "active session";
    }
    if config.storage.database_url.is_some() {
        return "bonhomme.toml";
    }
    "project-local default"
}

fn print_toolchains(config: &Config) {
    if config.toolchain.is_empty() {
        println!("- Toolchains: PATH defaults");
        return;
    }
    let configured = config
        .toolchain
        .iter()
        .map(|(name, command)| format!("{name}=`{command}`"))
        .collect::<Vec<_>>()
        .join(", ");
    println!("- Toolchains: {configured}");
}

fn print_formatters(config: &Config) {
    if config.format.is_empty() {
        println!("- Formatters: none configured");
        return;
    }
    let configured = config
        .format
        .iter()
        .map(|(extension, command)| format!("{extension}=`{command}`"))
        .collect::<Vec<_>>()
        .join(", ");
    println!("- Formatters: {configured}");
}

fn print_session_validation(config: &Config) {
    let note = if config.validation.session_start.toolchain_enabled() {
        "toolchain validation runs during session start"
    } else {
        "toolchain validation skipped during session start; graph invariants still checked"
    };
    println!(
        "- Session start validation: {} ({note})",
        config.validation.session_start.as_str()
    );
}

fn session_start_command(config: &Config) -> String {
    if config.validation.session_start.toolchain_enabled() {
        "bonhomme session start --reset --validate toolchain".to_string()
    } else {
        "bonhomme session start --reset".to_string()
    }
}

fn shell_word(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | '<' | '>'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}
