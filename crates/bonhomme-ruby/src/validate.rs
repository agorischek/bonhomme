use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_ruby_files(files: &[RenderedFile]) -> Result<()> {
    validate_ruby_files_with_ruby(files, None).await
}

pub async fn validate_ruby_files_with_ruby(
    files: &[RenderedFile],
    configured_ruby: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-ruby-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let mut ruby_paths = Vec::new();
    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        if file.path.ends_with(".rb") {
            ruby_paths.push(path);
        }
    }

    if ruby_paths.is_empty() {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let ruby = ruby_binary(configured_ruby);
    for path in &ruby_paths {
        let mut command = Command::new(&ruby);
        command.arg("-c").arg(path).current_dir(&root);
        let output = time::timeout(std::time::Duration::from_secs(30), command.output())
            .await
            .context("ruby -c timed out")?
            .context("failed to run ruby -c")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let _ = fs::remove_dir_all(&root).await;
            bail!("{ruby} rejected rendered output: {stderr}{stdout}");
        }
    }

    let _ = fs::remove_dir_all(&root).await;
    Ok(())
}

fn ruby_binary(configured_ruby: Option<&str>) -> String {
    std::env::var("BONHOMME_RUBY")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_ruby.and_then(non_empty))
        .unwrap_or_else(|| "ruby".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
