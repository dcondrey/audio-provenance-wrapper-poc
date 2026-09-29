//! A small bounded HTTP/1.1 client over `std::net`, with TLS through rustls.
//!
//! Used by the time-anchor providers (RFC 3161 TSAs and OpenTimestamps
//! calendars). The bounds are the point of it:
//!
//! - one overall deadline covers resolve, connect, TLS handshake, write and read;
//! - the body is capped, and one byte past the cap is kept so a caller can tell an
//!   oversized reply from one exactly at the bound;
//! - redirects are never followed. A 3xx is returned to the caller as a status and
//!   every caller treats it as a failure, so a hostile or misconfigured endpoint
//!   cannot bounce a request to another host, and https can never downgrade to
//!   http;
//! - certificate validation is always on. The trust anchors are the bundled
//!   Mozilla roots; [`HttpClient::with_additional_roots`] adds anchors for a test
//!   server and is not reachable from any command-line flag.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

const MAX_HEADER_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

pub struct HttpRequest<'a> {
    pub method: Method,
    pub url: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
    /// The body is truncated to `max_body + 1` bytes.
    pub max_body: usize,
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub status_line: String,
    pub body: Vec<u8>,
}

pub struct HttpClient {
    tls: Arc<ClientConfig>,
}

struct Target {
    tls: bool,
    host: String,
    port: u16,
    path: String,
    host_header: String,
}

fn parse_url(url: &str) -> Result<Target, String> {
    let (tls, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(format!("unsupported URL scheme in {url:?}: only http and https are spoken"));
    };
    let (authority, path) = match rest.find(['/', '?']) {
        Some(index) => {
            let (authority, tail) = rest.split_at(index);
            let path = if tail.starts_with('/') { tail.to_owned() } else { format!("/{tail}") };
            (authority, path)
        }
        None => (rest, "/".to_owned()),
    };
    if authority.is_empty()
        || authority.contains(['@', ' ', '\r', '\n'])
        || path.contains(['\r', '\n', ' '])
    {
        return Err(format!("invalid URL {url:?}"));
    }
    let port_split = if authority.starts_with('[') {
        authority.split_once("]:")
    } else {
        authority.rsplit_once(':')
    };
    let default_port = if tls { 443 } else { 80 };
    let (host, port) = match port_split {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| format!("invalid URL port in {url:?}"))?;
            (host, port)
        }
        None => (authority, default_port),
    };
    Ok(Target {
        tls,
        host: host.trim_start_matches('[').trim_end_matches(']').to_owned(),
        port,
        path,
        host_header: authority.to_owned(),
    })
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(stream) => stream,
            Stream::Tls(stream) => &stream.sock,
        }
    }

    fn set_deadline(&self, deadline: Instant) -> Result<(), String> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| "the request timed out".to_owned())?;
        self.tcp()
            .set_read_timeout(Some(remaining))
            .and_then(|()| self.tcp().set_write_timeout(Some(remaining)))
            .map_err(|error| error.to_string())
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(stream) => stream.read(buffer),
            Stream::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Plain(stream) => stream.write(buffer),
            Stream::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Plain(stream) => stream.flush(),
            Stream::Tls(stream) => stream.flush(),
        }
    }
}

fn client_config(roots: RootCertStore) -> Result<Arc<ClientConfig>, String> {
    // The provider is named explicitly so a second compiled-in provider could
    // never make the choice ambiguous at runtime.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| format!("cannot configure TLS: {error}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

impl HttpClient {
    /// Trusts the bundled Mozilla root set and nothing else.
    pub fn new() -> Result<HttpClient, String> {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Ok(HttpClient { tls: client_config(roots)? })
    }

    /// The bundled roots plus `extra`, for a test server with a self-signed
    /// certificate. Validation stays on: the extra certificates are trust
    /// anchors, not an exemption.
    pub fn with_additional_roots(extra: Vec<CertificateDer<'static>>) -> Result<HttpClient, String> {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for certificate in extra {
            roots
                .add(certificate)
                .map_err(|error| format!("unusable trust anchor: {error}"))?;
        }
        Ok(HttpClient { tls: client_config(roots)? })
    }

    pub fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let target = parse_url(request.url)?;
        let deadline = Instant::now() + request.timeout;
        let addresses = (target.host.as_str(), target.port)
            .to_socket_addrs()
            .map_err(|error| format!("cannot resolve the host: {error}"))?;
        let mut last_error = "the host has no addresses".to_owned();
        let mut connected = None;
        for address in addresses {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or_else(|| "the request timed out".to_owned())?;
            match TcpStream::connect_timeout(&address, remaining) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(error) => last_error = format!("cannot connect: {error}"),
            }
        }
        let tcp = connected.ok_or(last_error)?;
        let mut stream = if target.tls {
            let name = ServerName::try_from(target.host.clone())
                .map_err(|_| format!("{:?} is not a valid TLS server name", target.host))?;
            let connection = ClientConnection::new(Arc::clone(&self.tls), name)
                .map_err(|error| format!("cannot start TLS: {error}"))?;
            Stream::Tls(Box::new(StreamOwned::new(connection, tcp)))
        } else {
            Stream::Plain(tcp)
        };

        let method = match request.method {
            Method::Get => "GET",
            Method::Post => "POST",
        };
        let mut head = format!("{method} {} HTTP/1.1\r\nHost: {}\r\n", target.path, target.host_header);
        for (name, value) in request.headers {
            if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
                return Err("invalid request header".to_owned());
            }
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if request.method == Method::Post {
            head.push_str(&format!("Content-Length: {}\r\n", request.body.len()));
        }
        head.push_str("Connection: close\r\n\r\n");
        // The TLS handshake runs inside the first write, so the deadline is set
        // from the time that remains rather than from the full timeout.
        stream.set_deadline(deadline)?;
        stream
            .write_all(head.as_bytes())
            .and_then(|()| stream.write_all(request.body))
            .and_then(|()| stream.flush())
            .map_err(|error| format!("cannot send the request: {error}"))?;
        read_reply(&mut stream, deadline, request.max_body)
    }
}

/// Read the reply under the deadline and a hard size bound. Keeps at most
/// `MAX_HEADER_BYTES + max_body + 1` bytes.
fn read_reply(stream: &mut Stream, deadline: Instant, max_body: usize) -> Result<HttpResponse, String> {
    let limit = MAX_HEADER_BYTES + max_body + 1;
    let mut received: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        stream.set_deadline(deadline)?;
        let read = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            // A peer that closes without a TLS close_notify has still ended the
            // body; parse_reply checks it against Content-Length.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("cannot read the reply: {error}")),
        };
        received.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if received.len() >= limit {
            break;
        }
    }
    parse_reply(&received, max_body)
}

fn parse_reply(received: &[u8], max_body: usize) -> Result<HttpResponse, String> {
    let split = received
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "the reply has no complete HTTP header".to_owned())?;
    if split > MAX_HEADER_BYTES {
        return Err("the reply header exceeds the size bound".to_owned());
    }
    let head = String::from_utf8_lossy(received.get(..split).unwrap_or_default()).into_owned();
    let body = received.get(split + 4..).unwrap_or_default();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default().to_owned();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("the reply has no valid status line: {status_line:?}"))?;
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "the reply has an invalid Content-Length".to_owned())?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
    let payload = if chunked {
        decode_chunked(body, max_body)?
    } else {
        match content_length {
            // Reading stops at the size cap, so a body longer than the bound is
            // truncated to just past it; the caller reports it as oversized.
            Some(length) if body.len() < length && body.len() <= max_body => {
                return Err("the reply body is shorter than its Content-Length".to_owned());
            }
            Some(length) => body.get(..length.min(body.len())).unwrap_or_default().to_vec(),
            None => body.to_vec(),
        }
    };
    Ok(HttpResponse {
        status,
        status_line,
        body: payload.into_iter().take(max_body + 1).collect(),
    })
}

fn decode_chunked(mut body: &[u8], max_body: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "the reply has a truncated chunked body".to_owned())?;
        let size_text = String::from_utf8_lossy(body.get(..line_end).unwrap_or_default()).into_owned();
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| "the reply has an invalid chunk size".to_owned())?;
        body = body.get(line_end + 2..).unwrap_or_default();
        if size == 0 {
            return Ok(out);
        }
        let chunk = body
            .get(..size)
            .ok_or_else(|| "the reply has a truncated chunked body".to_owned())?;
        out.extend_from_slice(chunk);
        if out.len() > max_body {
            return Ok(out);
        }
        body = body.get(size + 2..).unwrap_or_default();
    }
}
