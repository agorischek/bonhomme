use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_elixir_files(files: &[RenderedFile]) -> Result<()> {
    validate_elixir_files_with_compiler(files, None).await
}

pub async fn validate_elixir_files_with_compiler(
    files: &[RenderedFile],
    configured_compiler: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-elixir-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let mut paths = Vec::new();
    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        if file.path.ends_with(".ex") || file.path.ends_with(".exs") {
            paths.push(path);
        }
    }

    if paths.is_empty() {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let compiler = elixir_compiler(configured_compiler);
    let mut command = Command::new(&compiler);
    command
        .arg("--ignore-module-conflict")
        .args(&paths)
        .current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context("elixir validation timed out")?
        .context("failed to run elixir validator")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("{compiler} rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn elixir_compiler(configured_compiler: Option<&str>) -> String {
    std::env::var("BONHOMME_ELIXIRC")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_compiler.and_then(non_empty))
        .unwrap_or_else(|| "elixirc".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
