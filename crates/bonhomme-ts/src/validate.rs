use anyhow::{Context, Result, bail};
use bonhomme_core::RenderedFile;
use serde_json::json;
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_typescript_files(files: &[RenderedFile]) -> Result<()> {
    validate_typescript_files_with_compiler(files, None).await
}

pub async fn validate_typescript_files_with_compiler(
    files: &[RenderedFile],
    configured_compiler: Option<&str>,
) -> Result<()> {
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
    fs::write(
        root.join("__bonhomme-jsx.d.ts"),
        "declare namespace JSX {\n  interface IntrinsicElements {\n    [elemName: string]: any;\n  }\n}\n",
    )
    .await?;

    let tsconfig = json!({
        "compilerOptions": {
            "target": "ES2022",
            "module": "ESNext",
            "allowJs": true,
            "jsx": "preserve",
            "strict": true,
            "noEmit": true,
            "skipLibCheck": true
        },
        "include": [
            "__bonhomme-jsx.d.ts",
            "repo/**/*.ts",
            "repo/**/*.tsx",
            "repo/**/*.js",
            "repo/**/*.jsx"
        ]
    });
    fs::write(
        root.join("tsconfig.json"),
        serde_json::to_string_pretty(&tsconfig)?,
    )
    .await?;

    let compiler = typescript_compiler(configured_compiler);
    let mut command = Command::new(&compiler);
    command
        .arg("--project")
        .arg(root.join("tsconfig.json"))
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false");

    command.current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(30), command.output()).await;

    let _ = fs::remove_dir_all(&root).await;

    let output = output
        .context("typescript compiler timed out")?
        .with_context(|| {
            format!(
                "failed to run TypeScript compiler `{compiler}`; install it locally or set \
                 BONHOMME_TSC / [toolchain].typescript"
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("{compiler} rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn typescript_compiler(configured_compiler: Option<&str>) -> String {
    std::env::var("BONHOMME_TSC")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_compiler.and_then(non_empty))
        .unwrap_or_else(|| "tsc".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
