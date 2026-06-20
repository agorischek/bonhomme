//! The formatter boundary. bonhomme renders a *canonical* projection of the semantic graph; when
//! those files land in a repo that has its own formatter, the canonical form would show up as
//! reformatting noise in the Git diff. This module runs a project-configured formatter over rendered
//! content at the write/compare seam, so a touched file matches the repo's style instead.
//!
//! Configured by the `[format]` table, keyed by file extension; the value is a shell command that
//! reads file content on stdin and writes the formatted result to stdout (e.g.
//! `rs = "rustfmt"`, `go = "gofmt"`, `ts = "prettier --stdin-filepath {path}"`). A `{path}`
//! placeholder is substituted with the file's path. Formatting is **best-effort and never loses
//! content**: an unconfigured extension, a binary file, or a failing formatter passes the content
//! through unchanged. It is applied symmetrically — on `land` (what gets written) and on the render
//! side of `check` (what the round-trip gate measures) — so the gate reflects what landing produces.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use bonhomme_core::{RenderedFile, decode_binary};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Format each file in place by its extension. Binary envelopes and unconfigured extensions pass
/// through untouched.
pub async fn format_files(
    mut files: Vec<RenderedFile>,
    formatters: &BTreeMap<String, String>,
) -> Vec<RenderedFile> {
    if formatters.is_empty() {
        return files;
    }
    for file in &mut files {
        // Binary content is a base64 envelope — never run a text formatter over it.
        if decode_binary(&file.content).is_some() {
            continue;
        }
        if let Some(formatted) =
            format_one(&file.content, Path::new(&file.path), formatters).await
        {
            file.content = formatted;
        }
    }
    files
}

/// Format a single string by `path`'s extension, returning `Some(formatted)` only when a formatter
/// is configured for the extension and runs successfully.
async fn format_one(
    content: &str,
    path: &Path,
    formatters: &BTreeMap<String, String>,
) -> Option<String> {
    let extension = path.extension().and_then(|extension| extension.to_str())?;
    let command = formatters.get(extension)?;
    let command = command.replace("{path}", &path.to_string_lossy());
    run_formatter(&command, content).await.ok()
}

async fn run_formatter(command: &str, content: &str) -> Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawning formatter `{command}`"))?;

    // Write stdin from a task while we read stdout, so a formatter that streams output cannot
    // deadlock against a full pipe.
    let mut stdin = child.stdin.take().context("formatter stdin unavailable")?;
    let input = content.to_string();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(input.as_bytes()).await;
        // stdin is dropped here, signalling EOF to the formatter.
    });
    let output = child.wait_with_output().await?;
    let _ = writer.await;

    if !output.status.success() {
        bail!("formatter `{command}` exited with {}", output.status);
    }
    String::from_utf8(output.stdout).context("formatter produced non-UTF-8 output")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str) -> RenderedFile {
        RenderedFile {
            path: path.to_string(),
            content: content.to_string(),
        }
    }

    #[tokio::test]
    async fn formats_configured_extension_and_passes_others_through() {
        // `tr a-z A-Z` is a stand-in formatter: reads stdin, uppercases, writes stdout.
        let mut formatters = BTreeMap::new();
        formatters.insert("up".to_string(), "tr a-z A-Z".to_string());

        let out = format_files(
            vec![file("a.up", "hello"), file("b.txt", "hello")],
            &formatters,
        )
        .await;

        assert_eq!(out[0].content, "HELLO"); // configured extension is formatted
        assert_eq!(out[1].content, "hello"); // unconfigured extension untouched
    }

    #[tokio::test]
    async fn failing_formatter_passes_content_through() {
        let mut formatters = BTreeMap::new();
        formatters.insert("x".to_string(), "exit 1".to_string());

        let out = format_files(vec![file("a.x", "keep me")], &formatters).await;

        assert_eq!(out[0].content, "keep me"); // never lose content on formatter failure
    }

    #[tokio::test]
    async fn empty_config_is_a_no_op() {
        let out = format_files(vec![file("a.up", "hello")], &BTreeMap::new()).await;
        assert_eq!(out[0].content, "hello");
    }
}
