use crate::model::{ParseRequest, ParsedPackage};
use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, safe_relative_path};
use std::{
    collections::{BTreeMap, hash_map::DefaultHasher},
    fs::{self as std_fs, Metadata},
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::SystemTime,
};
use tokio::{fs as tokio_fs, process::Command as TokioCommand, time};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct HelperCacheKey {
    command_name: String,
    input_len: usize,
    input_hash: u64,
}

static HELPER_OUTPUT_CACHE: OnceLock<Mutex<BTreeMap<HelperCacheKey, Vec<u8>>>> = OnceLock::new();

pub(crate) fn parse_go_files(files: &[RenderedFile]) -> Result<ParsedPackage> {
    if files.is_empty() {
        return Ok(ParsedPackage { files: Vec::new() });
    }
    let input = serde_json::to_vec(&ParseRequest { files })?;
    let output = run_helper("parse", &input)?;
    serde_json::from_slice(&output).context("failed to decode Go helper parse output")
}

pub(crate) fn format_go_source(source: &str) -> Result<String> {
    let output = run_helper("format", source.as_bytes())?;
    String::from_utf8(output).context("Go helper returned non-UTF-8 formatted source")
}

pub(crate) async fn validate_go_files(files: &[RenderedFile]) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }

    let root = std::env::temp_dir().join(format!("bonhomme-go-{}", Uuid::new_v4()));
    tokio_fs::create_dir_all(&root).await?;
    tokio_fs::write(
        root.join("go.mod"),
        "module bonhomme.local/rendered\n\ngo 1.22\n",
    )
    .await?;

    for file in files {
        let path = root.join(safe_relative_path(&file.path)?);
        if let Some(parent) = path.parent() {
            tokio_fs::create_dir_all(parent).await?;
        }
        tokio_fs::write(path, &file.content).await?;
    }

    let mut command = TokioCommand::new(go_binary());
    command.arg("build").arg("./...").current_dir(&root);
    let output = time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .context("go build timed out")?
        .context("failed to run go build")?;

    let _ = tokio_fs::remove_dir_all(&root).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("go build rejected rendered output: {stderr}{stdout}");
    }

    Ok(())
}

fn run_helper(command_name: &str, input: &[u8]) -> Result<Vec<u8>> {
    let key = helper_cache_key(command_name, input);
    if let Some(output) = cached_helper_output(&key) {
        return Ok(output);
    }

    let helper_binary = helper_binary()?;
    let mut child = Command::new(&helper_binary)
        .arg(command_name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start Go helper {}", helper_binary.display()))?;

    child
        .stdin
        .as_mut()
        .context("Go helper stdin is unavailable")?
        .write_all(input)?;

    let output = child
        .wait_with_output()
        .context("failed to wait for Go helper")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!("Go helper {command_name} failed: {stderr}{stdout}");
    }
    remember_helper_output(key, &output.stdout);
    Ok(output.stdout)
}

fn helper_cache_key(command_name: &str, input: &[u8]) -> HelperCacheKey {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    HelperCacheKey {
        command_name: command_name.to_string(),
        input_len: input.len(),
        input_hash: hasher.finish(),
    }
}

fn cached_helper_output(key: &HelperCacheKey) -> Option<Vec<u8>> {
    HELPER_OUTPUT_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(key).cloned())
}

fn remember_helper_output(key: HelperCacheKey, output: &[u8]) {
    if let Ok(mut cache) = HELPER_OUTPUT_CACHE
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
    {
        cache.insert(key, output.to_vec());
    }
}

fn helper_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("BONHOMME_GO_HELPER_BIN") {
        return Ok(PathBuf::from(path));
    }

    let helper_dir = helper_dir();
    let output_dir = std::env::temp_dir().join("bonhomme-go-helper");
    std_fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "failed to create Go helper cache directory {}",
            output_dir.display()
        )
    })?;

    let binary_path = output_dir.join(helper_binary_name(&helper_dir));
    if helper_needs_rebuild(&binary_path, &helper_dir)? {
        let output = Command::new(go_binary())
            .arg("build")
            .arg("-o")
            .arg(&binary_path)
            .arg(".")
            .current_dir(&helper_dir)
            .output()
            .with_context(|| format!("failed to build Go helper in {}", helper_dir.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            bail!("Go helper build failed: {stderr}{stdout}");
        }
    }

    Ok(binary_path)
}

fn helper_binary_name(helper_dir: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    helper_dir.hash(&mut hasher);
    format!(
        "bonhomme-go-helper-{}{}",
        hasher.finish(),
        std::env::consts::EXE_SUFFIX
    )
}

fn helper_needs_rebuild(binary_path: &Path, helper_dir: &Path) -> Result<bool> {
    let Ok(binary_metadata) = std_fs::metadata(binary_path) else {
        return Ok(true);
    };
    let binary_modified = modified_at(&binary_metadata)?;
    Ok(newest_helper_source_modified(helper_dir)? > binary_modified)
}

fn newest_helper_source_modified(helper_dir: &Path) -> Result<SystemTime> {
    let mut newest = SystemTime::UNIX_EPOCH;
    for entry in std_fs::read_dir(helper_dir).with_context(|| {
        format!(
            "failed to read Go helper directory {}",
            helper_dir.display()
        )
    })? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_source = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "go" | "mod" | "sum"));
        if is_source {
            newest = newest.max(modified_at(&entry.metadata()?)?);
        }
    }
    Ok(newest)
}

fn modified_at(metadata: &Metadata) -> Result<SystemTime> {
    metadata
        .modified()
        .context("filesystem did not report a modified timestamp")
}

fn go_binary() -> String {
    std::env::var("BONHOMME_GO").unwrap_or_else(|_| "go".to_string())
}

fn helper_dir() -> PathBuf {
    std::env::var("BONHOMME_GO_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("go-helper"))
}
