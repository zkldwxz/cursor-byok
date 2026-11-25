//! Fetches, verifies and splits the search program and its model for delivery.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use flate2::{write::GzEncoder, Compression};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::{
    local_app::workspace_extension::{vsix_asset_name, DEFAULT_RELEASE_URL},
    store::Store,
    Error, Result,
};

use super::command::Installed;

/// Keeps one write well below the frame sizes Cursor's transports accept.
const PART_BYTES: usize = 4 * 1024 * 1024;
const MODEL_DIRECTORY: &str = "semble/models/potion-code-16M-v2";
const MODEL_URL: &str = "https://huggingface.co/minishlab/potion-code-16M-v2/resolve/main";
const MODEL_FILES: [(&str, &str); 2] = [
    (
        "model.safetensors",
        "75cf7a6c2171b230ad19b1e7d8e0b1aee86da5a02af8e7cacedd9921d227623c",
    ),
    (
        "tokenizer.json",
        "107bbdcbad4bff1d299b7a4c3a2fb17c52890688b7dd0e4c9deab79d3c4f3d45",
    ),
];

/// One file ready to travel: the install step rejoins and verifies it.
#[derive(Debug)]
pub(super) struct Delivery {
    pub installed: Installed,
    pub parts: Vec<(String, Vec<u8>)>,
}

#[derive(Clone)]
pub struct SemanticPackage(Arc<Source>);

struct Source {
    version: String,
    release_url: String,
    model_url: String,
    model_files: Vec<(String, String)>,
    cache: Option<PathBuf>,
    store: Option<Store>,
    // Conversations that need the same download wait for it instead of repeating it.
    download: Mutex<()>,
}

impl SemanticPackage {
    pub fn managed(store: Store, version: String) -> Self {
        Self::official(Some(store), version)
    }

    /// Without persisted proxy settings.
    pub fn built_in() -> Self {
        Self::official(None, env!("CARGO_PKG_VERSION").into())
    }

    fn official(store: Option<Store>, version: String) -> Self {
        Self(Arc::new(Source {
            version,
            release_url: DEFAULT_RELEASE_URL.into(),
            model_url: MODEL_URL.into(),
            model_files: MODEL_FILES
                .map(|(name, sha256)| (name.into(), sha256.into()))
                .into(),
            cache: None,
            store,
            download: Mutex::new(()),
        }))
    }

    /// Test wiring: both hosts and the cache directory are given.
    pub fn with_sources(
        version: &str,
        release_url: String,
        model_url: String,
        model_files: Vec<(String, String)>,
        cache: PathBuf,
    ) -> Self {
        Self(Arc::new(Source {
            version: version.into(),
            release_url,
            model_url,
            model_files,
            cache: Some(cache),
            store: None,
            download: Mutex::new(()),
        }))
    }

    /// Names the installed program on the machine that holds the code.
    pub(super) fn installation(&self) -> String {
        installation(&self.0.version, option_env!("GITHUB_SHA"))
    }

    pub(super) async fn deliveries(
        &self,
        platform: &str,
        files: &[String],
    ) -> Result<Vec<Delivery>> {
        let _download = self.0.download.lock().await;
        let mut deliveries = Vec::new();
        for name in files {
            let bytes = if name == "program" {
                let vsix = self.vsix().await?;
                let platform = platform.to_string();
                blocking(move || program(&vsix, &platform)).await?
            } else {
                self.model_file(name).await?
            };
            let name = name.clone();
            deliveries.push(blocking(move || split(name, &bytes)).await?);
        }
        Ok(deliveries)
    }

    async fn vsix(&self) -> Result<Vec<u8>> {
        let name = vsix_asset_name(&self.0.version);
        let cached = self.cache()?.join("semantic").join(&name);
        let sidecar = cached.with_extension("vsix.sha256");
        if let (Ok(bytes), Ok(expected)) = (std::fs::read(&cached), std::fs::read(&sidecar)) {
            if checksum_line(&expected).is_some_and(|expected| digest(&bytes) == expected) {
                return Ok(bytes);
            }
        }
        let url = format!("{}/v{}/{name}", self.0.release_url, self.0.version);
        let line = self.fetch(&format!("{url}.sha256")).await?;
        let expected = checksum_line(&line)
            .ok_or_else(|| Error::Provider(format!("{url}.sha256 is not a sha256 line")))?;
        let bytes = self.verified(&url, &expected).await?;
        store(&cached, &bytes)?;
        store(&sidecar, &line)?;
        Ok(bytes)
    }

    async fn model_file(&self, name: &str) -> Result<Vec<u8>> {
        let (_, expected) = self
            .0
            .model_files
            .iter()
            .find(|(known, _)| known == name)
            .ok_or_else(|| Error::Protocol(format!("SemanticSearch has no file named {name}")))?;
        let cached = self.cache()?.join(MODEL_DIRECTORY).join(name);
        if let Ok(bytes) = std::fs::read(&cached) {
            if digest(&bytes) == *expected {
                return Ok(bytes);
            }
        }
        let bytes = self
            .verified(&format!("{}/{name}", self.0.model_url), expected)
            .await?;
        store(&cached, &bytes)?;
        Ok(bytes)
    }

    fn cache(&self) -> Result<PathBuf> {
        match &self.0.cache {
            Some(cache) => Ok(cache.clone()),
            None => Ok(crate::config::managed_data_dir()?.join("cache")),
        }
    }

    async fn verified(&self, url: &str, expected: &str) -> Result<Vec<u8>> {
        let bytes = self.fetch(url).await?;
        let actual = digest(&bytes);
        if actual != expected {
            return Err(Error::Provider(format!(
                "sha256 mismatch for {url}: expected {expected}, got {actual}"
            )));
        }
        Ok(bytes)
    }

    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let builder = match &self.0.store {
            Some(store) => crate::network::client_builder(store).await?,
            None => reqwest::Client::builder(),
        };
        let response = builder
            .timeout(std::time::Duration::from_secs(300))
            .build()?
            .get(url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| Error::Provider(format!("cannot download {url}: {error}")))?;
        let bytes = response
            .bytes()
            .await
            .map_err(|error| Error::Provider(format!("reading {url} failed: {error}")))?;
        Ok(bytes.to_vec())
    }
}

/// Test builds reuse a version before it is released, so a version alone would
/// leave an older build's program in place.
fn installation(version: &str, commit: Option<&str>) -> String {
    match commit.and_then(|commit| commit.get(..8)) {
        Some(commit) => format!("{version}-{commit}"),
        None => version.into(),
    }
}

fn program(vsix: &[u8], platform: &str) -> Result<Vec<u8>> {
    let path = format!("extension/bin/{platform}/byok-semantic");
    let failed = |error: String| Error::Provider(format!("the VSIX has no usable {path}: {error}"));
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(vsix)).map_err(|e| failed(e.to_string()))?;
    let mut file = archive
        .by_name(&path)
        .map_err(|error| failed(error.to_string()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| failed(error.to_string()))?;
    Ok(bytes)
}

fn split(name: String, bytes: &[u8]) -> Result<Delivery> {
    let sha256 = digest(bytes);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes)?;
    // The checksum in the name keeps the leftovers of an older delivery out.
    let parts = encoder
        .finish()?
        .chunks(PART_BYTES)
        .enumerate()
        .map(|(index, part)| (format!("{name}-{sha256}.{index:03}"), part.to_vec()))
        .collect();
    Ok(Delivery {
        installed: Installed { name, sha256 },
        parts,
    })
}

fn store(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let partial = path.with_extension(format!("part-{}", std::process::id()));
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, path)?;
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn checksum_line(line: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(line);
    let token = text.split_whitespace().next()?;
    (token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| Error::Provider(format!("preparing SemanticSearch failed: {error}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_checksums_are_the_ones_the_program_itself_verifies() {
        let program = include_str!("../../../../../crates/semble-core/src/embedding/assets.rs");
        for (name, sha256) in MODEL_FILES {
            assert!(program.contains(sha256), "{name}");
            assert!(
                program.contains(&format!("{MODEL_URL}/{name}")),
                "{name} must come from the same place"
            );
        }
    }

    #[test]
    fn two_builds_of_one_version_never_share_an_installed_program() {
        let first = installation("1.1.1", Some("9edc1de0aa"));
        assert_ne!(first, installation("1.1.1", Some("6c1db56f00")));
        assert_ne!(first, installation("1.1.1", None));
    }

    #[test]
    fn a_platform_the_vsix_does_not_carry_is_named_in_the_error() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                "extension/bin/linux-x64/byok-semantic",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"program").unwrap();
        let vsix = writer.finish().unwrap().into_inner();
        assert_eq!(program(&vsix, "linux-x64").unwrap(), b"program");
        let error = program(&vsix, "linux-arm64").unwrap_err().to_string();
        assert!(error.contains("linux-arm64"), "{error}");
    }

    #[test]
    fn a_large_file_travels_in_bounded_parts() {
        // Incompressible bytes, so the parts are as large as the file.
        let mut state = 1_u32;
        let bytes = (0..PART_BYTES * 2)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect::<Vec<_>>();
        let delivery = split("model.safetensors".into(), &bytes).unwrap();
        assert!(delivery.parts.len() >= 2);
        assert!(delivery
            .parts
            .iter()
            .all(|(_, part)| part.len() <= PART_BYTES));
    }
}
