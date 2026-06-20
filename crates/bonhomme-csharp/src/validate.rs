use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use tokio::{fs, process::Command, time};
use uuid::Uuid;

pub async fn validate_csharp_files(files: &[RenderedFile]) -> Result<()> {
    validate_csharp_files_with_dotnet(files, None).await
}

pub async fn validate_csharp_files_with_dotnet(
    files: &[RenderedFile],
    configured_dotnet: Option<&str>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-csharp-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).await?;

    let has_project = files
        .iter()
        .any(|file| file.path.ends_with(".csproj") || file.path.ends_with(".sln"));
    if !has_project {
        fs::write(root.join("BonhommeRendered.csproj"), synthetic_project()).await?;
    }

    let mut has_csharp = false;
    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &file.content).await?;
        has_csharp |= file.path.ends_with(".cs");
    }

    if !has_csharp {
        let _ = fs::remove_dir_all(&root).await;
        return Ok(());
    }

    let dotnet = dotnet_binary(configured_dotnet);
    let mut command = Command::new(&dotnet);
    command
        .arg("build")
        .arg("--nologo")
        .arg("--verbosity")
        .arg("quiet")
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .context("dotnet build timed out")?
        .context("failed to run dotnet build")?;

    let _ = fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("{dotnet} rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn synthetic_project() -> &'static str {
    r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net10.0</TargetFramework>
    <ImplicitUsings>disable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
</Project>
"#
}

fn dotnet_binary(configured_dotnet: Option<&str>) -> String {
    std::env::var("BONHOMME_DOTNET")
        .ok()
        .and_then(non_empty)
        .or_else(|| configured_dotnet.and_then(non_empty))
        .unwrap_or_else(|| "dotnet".to_string())
}

fn non_empty(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_string())
}
