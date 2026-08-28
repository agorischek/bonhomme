use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_java_files(files: &[RenderedFile]) -> Result<()> {
    validate_java_files_with_javac(files, None).await
}

pub async fn validate_java_files_with_javac(
    files: &[RenderedFile],
    configured_javac: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-java-{}", Uuid::new_v4()));
    let classes = root.join("classes");
    fs::create_dir_all(&classes).await?;

    let mut java_paths = Vec::new();
    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        if file.path.ends_with(".java") {
            java_paths.push(path);
        }
    }

    if java_paths.is_empty() {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let javac = javac_binary(configured_javac);
    let mut command = Command::new(&javac);
    command
        .arg("-d")
        .arg(&classes)
        .args(&java_paths)
        .current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .context("javac timed out")?
        .context("failed to run javac")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("{javac} rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn javac_binary(configured_javac: Option<&str>) -> String {
    std::env::var("BONHOMME_JAVAC")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_javac.and_then(non_empty))
        .unwrap_or_else(|| "javac".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
