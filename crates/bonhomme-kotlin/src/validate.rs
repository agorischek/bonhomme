use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_kotlin_files(files: &[RenderedFile]) -> Result<()> {
    validate_kotlin_files_with_kotlinc(files, None).await
}

pub async fn validate_kotlin_files_with_kotlinc(
    files: &[RenderedFile],
    configured_kotlinc: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-kotlin-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let mut kotlin_paths = Vec::new();
    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        if file.path.ends_with(".kt") || file.path.ends_with(".kts") {
            kotlin_paths.push(path);
        }
    }

    if kotlin_paths.is_empty() {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let kotlinc = kotlinc_binary(configured_kotlinc);
    let output_path = root.join("bonhomme-kotlin.jar");
    let mut command = Command::new(&kotlinc);
    command
        .args(&kotlin_paths)
        .arg("-d")
        .arg(&output_path)
        .current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .context("kotlinc timed out")?
        .context("failed to run kotlinc")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("{kotlinc} rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn kotlinc_binary(configured_kotlinc: Option<&str>) -> String {
    std::env::var("BONHOMME_KOTLINC")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_kotlinc.and_then(non_empty))
        .unwrap_or_else(|| "kotlinc".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
