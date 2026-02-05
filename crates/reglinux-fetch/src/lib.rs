use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FetchError(pub String);

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FetchError {}

impl From<reqwest::Error> for FetchError {
    fn from(err: reqwest::Error) -> Self {
        FetchError(err.to_string())
    }
}

impl From<std::io::Error> for FetchError {
    fn from(err: std::io::Error) -> Self {
        FetchError(err.to_string())
    }
}

impl From<serde_json::Error> for FetchError {
    fn from(err: serde_json::Error) -> Self {
        FetchError(err.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub schema: u32,
    pub repo: String,
    pub tag: String,
    pub target: String,
    pub image: String,
    pub bytes_uncompressed: Option<u64>,
    pub parts: Vec<Part>,
    pub sha256_zst: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub repo: String,
    pub tag: String,
    pub generated_at: String,
    pub images: HashMap<String, Meta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub assets: Vec<ReleaseAsset>,
}

pub fn build_client(token: Option<&str>) -> Result<Client, FetchError> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("reglinux-fetch"));
    if let Some(token) = token {
        let value = format!("token {token}");
        headers.insert("Authorization", HeaderValue::from_str(&value).map_err(|e| FetchError(e.to_string()))?);
    }
    Ok(Client::builder().default_headers(headers).build()?)
}

pub fn fetch_release(client: &Client, repo: &str, tag: &str) -> Result<Release, FetchError> {
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    let resp = client.get(url).send()?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(FetchError(format!("Release {tag} not found in {repo}")));
    }
    if !resp.status().is_success() {
        return Err(FetchError(format!("GitHub API error: {}", resp.status())));
    }
    Ok(resp.json::<Release>()?)
}

pub fn fetch_manifest(client: &Client, repo: &str, tag: &str) -> Result<Manifest, FetchError> {
    let release = fetch_release(client, repo, tag)?;
    let manifest_asset = release
        .assets
        .iter()
        .find(|asset| asset.name == "manifest.json")
        .ok_or_else(|| FetchError("manifest.json not found in release".to_string()))?;

    let resp = client.get(&manifest_asset.browser_download_url).send()?;
    if !resp.status().is_success() {
        return Err(FetchError(format!(
            "Failed to download manifest.json: {}",
            resp.status()
        )));
    }
    Ok(resp.json::<Manifest>()?)
}

pub fn sha256_file(path: &Path) -> Result<String, FetchError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn join_parts(parts: &[PathBuf], output: &Path) -> Result<(), FetchError> {
    let mut out = File::create(output)?;
    for part in parts {
        let mut input = File::open(part)?;
        std::io::copy(&mut input, &mut out)?;
    }
    out.flush()?;
    Ok(())
}

pub fn ensure_parent(path: &Path) -> Result<(), FetchError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_manifest() {
        let data = r#"{
          "schema":1,
          "repo":"REG-Linux/REG-Linux",
          "tag":"v1.0-rc1",
          "generated_at":"2024-01-01T00:00:00Z",
          "images":{
            "cha":{
              "schema":1,
              "repo":"REG-Linux/REG-Linux",
              "tag":"v1.0-rc1",
              "target":"cha",
              "image":"reglinux-cha-v1.0-rc1.img.zst",
              "parts":[{"name":"reglinux-cha-v1.0-rc1.img.zst.part000","size":10,"sha256":"abc"}],
              "created_at":"2024-01-01T00:00:00Z"
            }
          }
        }"#;
        let manifest: Manifest = serde_json::from_str(data).unwrap();
        assert_eq!(manifest.images.len(), 1);
        assert!(manifest.images.contains_key("cha"));
    }

    #[test]
    fn join_parts_in_order() {
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("part1");
        let p2 = dir.path().join("part2");
        fs::write(&p1, b"hello ").unwrap();
        fs::write(&p2, b"world").unwrap();
        let out = dir.path().join("out");
        join_parts(&[p1, p2], &out).unwrap();
        let data = fs::read(out).unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn sha256_matches() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("blob");
        fs::write(&path, b"reglinux").unwrap();
        let hash = sha256_file(&path).unwrap();
        assert_eq!(hash, "0c73cd147f461a1c4b24de90fbe6456abc0f69d470ef93937cfede381b4919ae");
    }
}
