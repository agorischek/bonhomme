use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_php_files(files: &[RenderedFile]) -> Result<()> {
    validate_php_files_with_php(files, None).await
}

pub async fn validate_php_files_with_php(
    files: &[RenderedFile],
    configured_php: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-php-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let mut php_paths = Vec::new();
    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        if file.path.ends_with(".php") {
            php_paths.push(path);
        }
    }

    if php_paths.is_empty() {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let php = php_binary(configured_php);
    for path in &php_paths {
        let mut command = Command::new(&php);
        command.arg("-l").arg(path).current_dir(&root);
        let output = time::timeout(std::time::Duration::from_secs(30), command.output())
            .await
            .context("php -l timed out")?
            .context("failed to run php -l")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let _ = fs::remove_dir_all(&root).await;
            bail!("{php} rejected rendered output: {stderr}{stdout}");
        }
    }

    let _ = fs::remove_dir_all(&root).await;
    Ok(())
}

fn php_binary(configured_php: Option<&str>) -> String {
    std::env::var("BONHOMME_PHP")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_php.and_then(non_empty))
        .unwrap_or_else(|| "php".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
