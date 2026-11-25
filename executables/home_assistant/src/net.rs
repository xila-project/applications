//! HTTP over the Xila network devices, for the Home Assistant REST API.
//!
//! A request is written whole to `/devices/network/https_client` (or
//! `http_client`); the device answers with the response head followed by the
//! decoded body, which is read through a small buffer.

use crate::error::{Error, Result};
use crate::json::Source;
use std::{format, string::String, vec, vec::Vec};

const HEAD_CAPACITY: usize = 4 * 1024;
/// Largest response body read; a longer one fails as too large. A Home
/// Assistant state answer is a few kilobytes, so this only stops a server
/// that never ends its answer.
pub const MAXIMUM_BODY_LENGTH: usize = 2 * 1024 * 1024;
const USER_AGENT: &str = "Xila-Home-Assistant";

/// Reads response bytes; `Ok(0)` is the end of the response.
pub trait Transport {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize>;
}

/// Sends a whole request and returns the transport its response is read from.
pub trait Connector {
    type Transport: Transport;
    fn send(&mut self, device: &str, request: &[u8]) -> Result<Self::Transport>;
}

/// Home Assistant's base URL, split for requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    secure: bool,
    host: String,
    /// Path prefix without a trailing slash, for a sub-path install.
    base_path: String,
}

impl Endpoint {
    pub fn parse(url: &str) -> Result<Self> {
        crate::configuration::validate_url(url).map_err(|_| Error::Configuration)?;
        let (secure, rest) = match url.strip_prefix("https://") {
            Some(rest) => (true, rest),
            None => (
                false,
                url.strip_prefix("http://").ok_or(Error::Configuration)?,
            ),
        };
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        Ok(Self {
            secure,
            host: String::from(host),
            base_path: format!("/{}", path.trim_end_matches('/'))
                .trim_end_matches('/')
                .into(),
        })
    }

    pub fn device(&self) -> &'static str {
        if self.secure {
            "/devices/network/https_client"
        } else {
            "/devices/network/http_client"
        }
    }

    /// The request bytes for `path`, below the base path.
    pub fn request(&self, method: &str, path: &str, token: &str, body: Option<&[u8]>) -> Vec<u8> {
        let mut head = format!(
            "{method} {}{path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\nAccept-Encoding: identity\r\nUser-Agent: {USER_AGENT}\r\nAuthorization: Bearer {token}\r\n",
            self.base_path, self.host
        );
        if let Some(body) = body {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            ));
        }
        head.push_str("\r\n");
        let mut request = head.into_bytes();
        if let Some(body) = body {
            request.extend_from_slice(body);
        }
        request
    }
}

/// Body of a response, read through the buffer that held its head.
pub struct Body<T> {
    transport: T,
    buffer: Vec<u8>,
    position: usize,
    length: usize,
    /// Body bytes taken from the transport so far, the head's leftover included.
    received: usize,
}

impl<T: Transport> Source for Body<T> {
    fn next_byte(&mut self) -> Result<Option<u8>> {
        if self.position == self.length {
            self.length = self.transport.read(&mut self.buffer)?;
            self.position = 0;
            if self.length == 0 {
                return Ok(None);
            }
            self.received = self.received.saturating_add(self.length);
            if self.received > MAXIMUM_BODY_LENGTH {
                return Err(Error::TooLarge);
            }
        }
        let byte = self.buffer[self.position];
        self.position += 1;
        Ok(Some(byte))
    }
}

/// End of the head and start of the body, for CRLF or bare LF line ends.
fn head_end(bytes: &[u8]) -> Option<(usize, usize)> {
    for index in 0..bytes.len() {
        if bytes[index..].starts_with(b"\r\n\r\n") {
            return Some((index, index + 4));
        }
        if bytes[index..].starts_with(b"\n\n") {
            return Some((index, index + 2));
        }
    }
    None
}

fn status(head: &[u8]) -> Option<u16> {
    let line = head.split(|byte| *byte == b'\n').next()?;
    let line = core::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

/// Reads the head of a response and returns its status and body.
pub fn read_response<T: Transport>(mut transport: T) -> Result<(u16, Body<T>)> {
    let mut buffer = vec![0; HEAD_CAPACITY];
    let mut filled = 0;
    let (head, body) = loop {
        if let Some(found) = head_end(&buffer[..filled]) {
            break found;
        }
        if filled == buffer.len() {
            return Err(Error::TooLarge);
        }
        let count = transport.read(&mut buffer[filled..])?;
        if count == 0 {
            return Err(Error::InvalidResponse);
        }
        filled += count;
    };
    let status = status(&buffer[..head]).ok_or(Error::InvalidResponse)?;
    Ok((
        status,
        Body {
            transport,
            buffer,
            position: body,
            length: filled,
            received: filled - body,
        },
    ))
}

/// Errors for statuses no caller handles.
pub fn status_error(status: u16) -> Error {
    match status {
        401 | 403 => Error::Unauthorized,
        404 => Error::NotFound,
        status => Error::Http(status),
    }
}

/// The network devices of a Xila system.
#[cfg(target_arch = "wasm32")]
pub mod device {
    use super::{Connector, Transport};
    use crate::error::{Error, Result};
    use std::{
        fs::{File, OpenOptions},
        io::{ErrorKind, Read, Write},
        thread::sleep,
        time::{Duration, Instant},
    };

    /// How long a read may keep reporting that no data is ready.
    const READ_TIMEOUT: Duration = Duration::from_secs(30);
    /// Longest wait between two reads of a device that has no data yet.
    const MAXIMUM_DELAY: Duration = Duration::from_millis(16);
    /// Xila's "resource busy" error number.
    const RESOURCE_BUSY: i32 = 277;

    pub struct Device(File);

    impl Transport for Device {
        fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
            let start = Instant::now();
            let mut attempt = 0u32;
            loop {
                match self.0.read(buffer) {
                    Ok(count) => return Ok(count),
                    Err(error)
                        if matches!(
                            error.kind(),
                            ErrorKind::WouldBlock | ErrorKind::Interrupted
                        ) || error.raw_os_error() == Some(RESOURCE_BUSY) =>
                    {
                        if start.elapsed() >= READ_TIMEOUT {
                            return Err(Error::Timeout);
                        }
                        // The guest cannot wait for readiness: sleep, 1 ms
                        // doubling up to 16 ms, so other tasks run.
                        sleep(Duration::from_millis(1 << attempt.min(4)).min(MAXIMUM_DELAY));
                        attempt += 1;
                    }
                    Err(error) if error.kind() == ErrorKind::TimedOut => {
                        return Err(Error::Timeout);
                    }
                    Err(_) => return Err(Error::Network),
                }
            }
        }
    }

    pub struct Devices;

    impl Connector for Devices {
        type Transport = Device;

        fn send(&mut self, device: &str, request: &[u8]) -> Result<Device> {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(device)
                .map_err(|_| Error::Network)?;
            // The device takes a whole request per write.
            file.write_all(request).map_err(|_| Error::Network)?;
            Ok(Device(file))
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Serves a canned response in fragments of `step` bytes.
    pub struct Canned {
        pub bytes: Vec<u8>,
        pub position: usize,
        pub step: usize,
    }

    impl Transport for Canned {
        fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
            let count = buffer
                .len()
                .min(self.step)
                .min(self.bytes.len() - self.position);
            buffer[..count].copy_from_slice(&self.bytes[self.position..self.position + count]);
            self.position += count;
            Ok(count)
        }
    }

    pub fn canned(text: &str, step: usize) -> Canned {
        Canned {
            bytes: text.as_bytes().to_vec(),
            position: 0,
            step,
        }
    }

    #[test]
    fn endpoints_keep_scheme_host_and_sub_path() {
        let endpoint = Endpoint::parse("http://homeassistant.local:8123").unwrap();
        assert_eq!(endpoint.device(), "/devices/network/http_client");
        assert_eq!(
            endpoint.request("GET", "/api/states/light.living_room", "secret", None),
            b"GET /api/states/light.living_room HTTP/1.1\r\nHost: homeassistant.local:8123\r\nConnection: close\r\nAccept: application/json\r\nAccept-Encoding: identity\r\nUser-Agent: Xila-Home-Assistant\r\nAuthorization: Bearer secret\r\n\r\n"
        );
        let endpoint = Endpoint::parse("https://homeassistant.example.com:8123/hass/").unwrap();
        assert_eq!(endpoint.device(), "/devices/network/https_client");
        let request = endpoint.request("POST", "/api/services/light/toggle", "t", Some(b"{}"));
        let text = core::str::from_utf8(&request).unwrap();
        assert!(
            text.starts_with("POST /hass/api/services/light/toggle HTTP/1.1\r\nHost: homeassistant.example.com:8123\r\n")
        );
        assert!(text.ends_with("Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"));
        assert_eq!(
            Endpoint::parse("homeassistant.local:8123"),
            Err(Error::Configuration)
        );
    }

    /// A response whose body never ends.
    struct Endless {
        head_sent: bool,
    }

    impl Transport for Endless {
        fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
            if !self.head_sent {
                self.head_sent = true;
                let head = b"HTTP/1.1 200 OK\r\n\r\n";
                buffer[..head.len()].copy_from_slice(head);
                return Ok(head.len());
            }
            buffer.fill(b' ');
            Ok(buffer.len())
        }
    }

    #[test]
    fn a_body_that_never_ends_stops_at_the_limit() {
        let (status, mut body) = read_response(Endless { head_sent: false }).unwrap();
        assert_eq!(status, 200);
        let mut count = 0usize;
        let error = loop {
            match body.next_byte() {
                Ok(Some(_)) => count += 1,
                Ok(None) => panic!("the body ended"),
                Err(error) => break error,
            }
        };
        assert_eq!(error, Error::TooLarge);
        assert!(count <= MAXIMUM_BODY_LENGTH);
    }

    #[test]
    fn responses_split_at_any_read_boundary() {
        let text = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}";
        for step in 1..text.len() {
            let (status, mut body) = read_response(canned(text, step)).unwrap();
            assert_eq!(status, 200);
            let mut bytes = Vec::new();
            while let Some(byte) = body.next_byte().unwrap() {
                bytes.push(byte);
            }
            assert_eq!(bytes, b"{\"ok\":true}");
        }
        let (status, _) = read_response(canned("HTTP/1.0 401 Unauthorized\n\n", 3)).unwrap();
        assert_eq!(status, 401);
        assert!(matches!(
            read_response(canned("HTTP/1.1 200 OK\r\n", 4)),
            Err(Error::InvalidResponse)
        ));
        assert!(matches!(
            read_response(canned("garbage\r\n\r\n", 4)),
            Err(Error::InvalidResponse)
        ));
        let huge = std::format!(
            "HTTP/1.1 200 OK\r\nX: {}\r\n\r\n",
            "a".repeat(HEAD_CAPACITY)
        );
        assert!(matches!(
            read_response(canned(&huge, 512)),
            Err(Error::TooLarge)
        ));
        assert_eq!(status_error(403), Error::Unauthorized);
        assert_eq!(status_error(502), Error::Http(502));
    }
}
