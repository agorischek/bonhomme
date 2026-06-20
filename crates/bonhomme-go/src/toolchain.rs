use crate::model::{ParseRequest, ParsedPackage};
use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tokio::{fs, process::Command as TokioCommand, time};
use uuid::Uuid;

pub(crate) fn parse_go_files(files: &[RenderedFile]) -> Result<ParsedPackage> {
    if files.is_empty() {
        return Ok(ParsedPackage { files: Vec::new() });
    }
    let input = serde_json::to_vec(&ParseRequest { files })?;
    let output = run_helper("parse", &input)?;
    serde_json::from_slice(&output).context("failed to decode Go helper parse output")
}

pub(crate) fn format_go_source(source: &str) -> Result<String> {
    let output = run_helper("format", source.as_bytes())?;
    String::from_utf8(output).context("Go helper returned non-UTF-8 formatted source")
}

pub(crate) async fn validate_go_files(files: &[RenderedFile]) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-go-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;
    fs::write(
        root.join("go.mod"),
        "module bonhomme.local/rendered\n\ngo 1.22\n",
    )
    .await?;

    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(path, &file.content).await?;
    }

    let mut command = TokioCommand::new(go_binary());
    command.arg("build").arg("./...").current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context("go build timed out")?
        .context("failed to run go build")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("go build rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn run_helper(command_name: &str, input: &[u8]) -> Result<Vec<u8>> {
    let helper_dir = helper_dir();
    let mut child = Command::new(go_binary())
        .arg("run")
        .arg(".")
        .arg(command_name)
        .current_dir(&helper_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start Go helper in {}", helper_dir.display()))?;

    child
        .stdin
        .as_mut()
        .context("Go helper stdin is unavailable")?
        .write_all(input)?;

    let output = child
        .wait_with_output()
        .context("failed to wait for Go helper")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("Go helper {command_name} failed: {stderr}{stdout}");
    }
    Ok(output.stdout)
}

fn go_binary() -> String {
    std::env::var("BONHOMME_GO").unwrap_or_else(|_| "go".to_string())
}

fn helper_dir() -> PathBuf {
    std::env::var("BONHOMME_GO_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("go-helper"))
}
