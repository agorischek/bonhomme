use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use std::path::Path;
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_rust_files(files: &[RenderedFile]) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-rust-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let has_manifest = files.iter().any(|file| file.path == "Cargo.toml");
    if !has_manifest {
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"bonhomme-rendered\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .await?;
    }

    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(path, &file.content).await?;
    }

    ensure_target_exists(&root, files).await?;

    let mut command = Command::new(cargo_binary());
    command.arg("check").arg("--quiet").current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context("cargo check timed out")?
        .context("failed to run cargo check")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("cargo check rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

async fn ensure_target_exists(root: &Path, files: &[RenderedFile]) -> Result<()> {
    let has_lib = files.iter().any(|file| file.path == "src/lib.rs");
    let has_main = files.iter().any(|file| file.path == "src/main.rs");
    if has_lib || has_main {
        return Ok(());
    }
    fs::create_dir_all(root.join("src")).await?;
    fs::write(root.join("src/lib.rs"), synthetic_lib(files)?).await?;
    Ok(())
}

fn synthetic_lib(files: &[RenderedFile]) -> Result<String> {
    let mut modules = String::new();
    for (index, file) in files
        .iter()
        .filter(|file| file.path.ends_with(".rs"))
        .enumerate()
    {
        let path = if let Some(stripped) = file.path.strip_prefix("src/") {
            stripped.to_string()
        } else {
            format!("../{}", file.path)
        };
        let quoted_path = serde_json::to_string(&path)?;
        modules.push_str(&format!(
            "#[path = {quoted_path}]\nmod bonhomme_file_{index};\n"
        ));
    }
    Ok(modules)
}

fn cargo_binary() -> String {
    std::env::var("BONHOMME_CARGO").unwrap_or_else(|_| "cargo".to_string())
}
