use anyhow::{Context, Result, bail};
use bonhomme_core::RenderedFile;
use serde_json::json;
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_typescript_files(files: &[RenderedFile]) -> Result<()> {
    let root = std::env::temp_dir().join(format!("bonhomme-tsc-{}", Uuid::new_v4()));
    let src_root = root.join("repo");
    fs::create_dir_all(&src_root).await?;

    for file in files {
        let path = src_root.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(path, &file.content).await?;
    }

    let tsconfig = json!({
        "compilerOptions": {
            "target": "ES2022",
            "module": "ESNext",
            "strict": true,
            "noEmit": true,
            "skipLibCheck": true
        },
        "include": ["repo/**/*.ts"]
    });
    fs::write(
        root.join("tsconfig.json"),
        serde_json::to_string_pretty(&tsconfig)?,
    )
    .await?;

    let mut command = if let Ok(tsc) = std::env::var("BONHOMME_TSC") {
        let mut command = Command::new(tsc);
        command
            .arg("--project")
            .arg(root.join("tsconfig.json"))
            .arg("--noEmit")
            .arg("--pretty")
            .arg("false");
        command
    } else {
        let mut command = Command::new("npx");
        command
            .arg("--yes")
            .arg("-p")
            .arg("typescript")
            .arg("tsc")
            .arg("--project")
            .arg(root.join("tsconfig.json"))
            .arg("--noEmit")
            .arg("--pretty")
            .arg("false");
        command
    };

    command.current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context("typescript compiler timed out")?
        .context("failed to run TypeScript compiler")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("tsc rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}
