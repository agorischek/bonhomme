use anyhow::{Context, Result, bail};
use bonhomme_core::RenderedFile;
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_python_files(files: &[RenderedFile]) -> Result<()> {
    validate_python_files_with_interpreter(files, None).await
}

pub async fn validate_python_files_with_interpreter(
    files: &[RenderedFile],
    configured_interpreter: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-python-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let mut paths = Vec::new();
    for file in files {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        if file.path.ends_with(".py") || file.path.ends_with(".pyi") {
            paths.push(path);
        }
    }

    if paths.is_empty() {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let interpreter = python_binary(configured_interpreter);
    let mut command = Command::new(&interpreter);
    command.arg("-m").arg("py_compile").args(&paths);
    let output = time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context("python validation timed out")?
        .context("failed to run python validator")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("{interpreter} rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn python_binary(configured_interpreter: Option<&str>) -> String {
    std::env::var("BONHOMME_PYTHON")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_interpreter.and_then(non_empty))
        .unwrap_or_else(|| "python3".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
