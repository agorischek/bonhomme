use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_swift_files(files: &[RenderedFile]) -> Result<()> {
    validate_swift_files_with_swiftc(files, None).await
}

pub async fn validate_swift_files_with_swiftc(
    files: &[RenderedFile],
    configured_swiftc: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-swift-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let mut swift_paths = Vec::new();
    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        if file.path.ends_with(".swift") {
            swift_paths.push(path);
        }
    }

    if swift_paths.is_empty() {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let swiftc = swiftc_binary(configured_swiftc);
    let mut command = Command::new(&swiftc);
    command.arg("-parse").args(&swift_paths).current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .context("swiftc -parse timed out")?
        .context("failed to run swiftc -parse")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("{swiftc} rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn swiftc_binary(configured_swiftc: Option<&str>) -> String {
    std::env::var("BONHOMME_SWIFTC")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_swiftc.and_then(non_empty))
        .unwrap_or_else(|| "swiftc".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
