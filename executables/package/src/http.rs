//! Minimal HTTPS GET over the `/devices/network/https_client` device.
//!
//! The device does not authenticate the server (no certificate validation), so
//! nothing fetched here is trusted on its own: registry metadata is verified
//! from the repository's root key and archives by their SHA-256.

use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use xila::{
    file_system::{AccessFlags, Path},
    shared::{HttpResponseParser, Url},
    task::TaskIdentifier,
    virtual_file_system::{File, VirtualFileSystem},
};

const HTTPS_CLIENT: &str = "/devices/network/https_client";
const MAX_REDIRECTS: usize = 5;
/// The device publishes the response head in one buffer of this size.
const HEAD_BUFFER: usize = 2048;
const CHUNK: usize = 2048;

/// Receives the response body in chunks.
pub trait Sink {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// Collects a body of at most `limit` bytes.
pub struct Memory {
    pub bytes: Vec<u8>,
    limit: usize,
}

impl Memory {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

impl Sink for Memory {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.bytes.len() + bytes.len() > self.limit {
            return Err(format!("response larger than {} bytes", self.limit));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

fn request(url: &Url<'_>) -> String {
    format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: xila-package\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.path, url.host
    )
}

/// Resolve a `Location` header against the current URL.
fn redirect(current: &Url<'_>, location: &str) -> Result<String, String> {
    if location.starts_with("https://") {
        Ok(location.to_string())
    } else if location.starts_with('/') && !location.starts_with("//") {
        Ok(format!("https://{}{location}", current.host))
    } else {
        Err(format!("refusing redirect to {location}"))
    }
}

enum Outcome {
    Done,
    Redirect(String),
}

async fn exchange(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    url: &str,
    sink: &mut impl Sink,
) -> Result<Outcome, String> {
    let parsed = Url::parse(url).ok_or_else(|| format!("invalid URL {url}"))?;
    if parsed.scheme != "https" || parsed.host.is_empty() {
        return Err(format!("only HTTPS URLs are fetched: {url}"));
    }
    let mut device = File::open(
        vfs,
        task,
        Path::from_str(HTTPS_CLIENT),
        AccessFlags::READ_WRITE.into(),
    )
    .await
    .map_err(|e| format!("cannot open {HTTPS_CLIENT}: {e}"))?;
    let result = async {
        let request = request(&parsed);
        let mut written = 0;
        while written < request.len() {
            let count = device
                .write(&request.as_bytes()[written..])
                .await
                .map_err(|e| format!("request to {} failed: {e}", parsed.host))?;
            if count == 0 {
                return Err("request write stalled".to_string());
            }
            written += count;
        }

        // The head is published on its own; read until its blank line.
        let mut head = vec![0u8; HEAD_BUFFER];
        let mut filled = 0;
        while !head[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
            if filled == head.len() {
                return Err("response head too large".to_string());
            }
            let count = device
                .read(&mut head[filled..])
                .await
                .map_err(|e| format!("response from {} failed: {e}", parsed.host))?;
            if count == 0 {
                return Err("connection closed before the response head".to_string());
            }
            filled += count;
        }
        let response = HttpResponseParser::from_buffer(&head[..filled]);
        let status = response
            .get_status_code()
            .ok_or_else(|| "invalid HTTP status line".to_string())?;
        match status {
            200 => {}
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .get_headers()
                    .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                    .map(|(_, value)| value.trim())
                    .ok_or_else(|| "redirect without Location".to_string())?;
                return Ok(Outcome::Redirect(redirect(&parsed, location)?));
            }
            404 => return Err(format!("not found: {url}")),
            other => return Err(format!("HTTP {other} for {url}")),
        }

        let mut chunk = vec![0u8; CHUNK];
        loop {
            let count = device
                .read(&mut chunk)
                .await
                .map_err(|e| format!("download from {} failed: {e}", parsed.host))?;
            if count == 0 {
                return Ok(Outcome::Done);
            }
            sink.write(&chunk[..count]).await?;
        }
    }
    .await;
    let close = device
        .close(vfs)
        .await
        .map_err(|e| format!("cannot close {HTTPS_CLIENT}: {e}"));
    match (result, close) {
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(e),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

/// GET `url` into `sink`, following up to five HTTPS redirects.
pub async fn get(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    url: &str,
    sink: &mut impl Sink,
) -> Result<(), String> {
    let mut current = url.to_string();
    for _ in 0..=MAX_REDIRECTS {
        match exchange(vfs, task, &current, sink).await? {
            Outcome::Done => return Ok(()),
            Outcome::Redirect(next) => current = next,
        }
    }
    Err(format!("too many redirects for {url}"))
}

/// GET a document of at most `limit` bytes.
pub async fn get_bytes(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    url: &str,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut memory = Memory::new(limit);
    get(vfs, task, url, &mut memory).await?;
    Ok(memory.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_closed_get_request() {
        let url = Url::parse("https://registry.xila.dev/root.json").unwrap();
        assert_eq!(
            request(&url),
            "GET /root.json HTTP/1.1\r\nHost: registry.xila.dev\r\nUser-Agent: xila-package\r\nAccept: */*\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn follows_only_https_redirects() {
        let url = Url::parse("https://github.com/a/b").unwrap();
        assert_eq!(
            redirect(&url, "https://objects.example/x").unwrap(),
            "https://objects.example/x"
        );
        assert_eq!(redirect(&url, "/c").unwrap(), "https://github.com/c");
        assert!(redirect(&url, "http://objects.example/x").is_err());
        assert!(redirect(&url, "//objects.example/x").is_err());
        assert!(redirect(&url, "relative").is_err());
    }
}
