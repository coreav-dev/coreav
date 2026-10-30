//! Reading HTTP(S) URLs by byte ranges (`docs/design/remote.md`): the
//! native host of `coreav-range`'s read plan, and the `Source` the
//! commands read through.
//!
//! Each request runs on its own thread, so that a pipeline's segments are
//! fetched at once; a thread reads its response into a bounded queue (the
//! plan's read-ahead) and stops when the request is cancelled. Redirects
//! are followed only to HTTPS URLs. With [`Options::guarded`] (the Python
//! module's default) a request goes only to public addresses: every address a host
//! name resolves to is checked, the connection is made to a checked address
//! (so a second, different DNS answer cannot redirect it), proxies are not
//! used, and only HTTPS is accepted.

use crate::error::Failure;
use coreav_isobmff::{Error, Source};
use coreav_range::{Host, HostError, Reader, Request, Response, Settings};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

/// Bytes a request thread reads at once.
const CHUNK: usize = 64 << 10;

/// Redirects one request may follow.
const MAX_REDIRECTS: u32 = 5;

/// How remote objects are read.
#[derive(Clone, Debug)]
pub struct Options {
    /// The read plan's numbers.
    pub settings: Settings,
    /// Whether private, loopback, link-local and other non-public addresses
    /// may be reached.
    pub private_network: bool,
    /// Whether a URL may start with `http://` (redirects always need
    /// `https://`).
    pub plain_http: bool,
    /// Whether the proxy variables of the environment (`HTTPS_PROXY` and
    /// others) are used.
    pub proxy_from_env: bool,
    /// How long to wait for a connection.
    pub connect_timeout: Duration,
    /// How long to wait for a response's head, or for the next bytes of a
    /// body being read.
    pub idle_timeout: Duration,
    /// How long receiving one response's body may take, from its head on
    /// (a server that sends a byte now and then cannot hold a request open
    /// for ever).
    pub body_timeout: Duration,
    /// How long one object may be read in all, if limited.
    pub deadline: Option<Duration>,
    /// Bytes of the object kept in a temporary file, so that later passes
    /// over the same bytes (a remux lists the samples, then copies them) do
    /// not fetch them again; 0 for none.
    pub spool_bytes: u64,
}

impl Options {
    /// The command line's: any address (a server on the local network is a
    /// normal input), `http://` too, the environment's proxies.
    pub fn command() -> Self {
        Self {
            settings: Settings::NATIVE,
            private_network: true,
            plain_http: true,
            proxy_from_env: true,
            connect_timeout: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(60),
            body_timeout: Duration::from_secs(300),
            deadline: None,
            spool_bytes: 4 << 30,
        }
    }

    /// The command line's options, with the read plan's numbers tuned by
    /// `COREAV_REMOTE_SETTINGS`.
    pub fn from_env() -> Self {
        let mut options = Self::command();
        if let Ok(text) = std::env::var("COREAV_REMOTE_SETTINGS") {
            tune(&mut options.settings, &text);
        }
        options
    }

    /// For URLs another program chose (the Python module's default): public
    /// addresses only, HTTPS only, no proxies, and at most 16 GiB received
    /// per object.
    pub fn guarded() -> Self {
        Self {
            settings: Settings {
                max_bytes: 16 << 30,
                ..Settings::NATIVE
            },
            private_network: false,
            plain_http: false,
            proxy_from_env: false,
            spool_bytes: 2 << 30,
            deadline: Some(Duration::from_secs(3600)),
            ..Self::command()
        }
    }
}

/// Applies `name=value` pairs, comma separated, to the plan's numbers: the
/// measurement knob `COREAV_REMOTE_SETTINGS` (command line only; unknown
/// names and bad values are ignored).
fn tune(settings: &mut Settings, text: &str) {
    for pair in text.split(',') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<u64>() else {
            continue;
        };
        let small = usize::try_from(value).unwrap_or(usize::MAX);
        match name.trim() {
            "first_span" => settings.first_span = value,
            "max_skip" => settings.max_skip = value,
            "cache_bytes" => settings.cache_bytes = value,
            "parallel" => settings.parallel = small,
            "segment_bytes" => settings.segment_bytes = value,
            "buffer_bytes" => settings.buffer_bytes = value,
            "keep_behind" => settings.keep_behind = value,
            "max_pipelines" => settings.max_pipelines = small,
            "open_bodies" => settings.open_bodies = small,
            _ => {}
        }
    }
}

/// Whether an input names a URL rather than a file: it starts with
/// `http://` or `https://` (any case), or another scheme followed by `://`,
/// which [`open`] refuses.
pub fn is_url(input: &str) -> bool {
    match input.split_once("://") {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
        None => false,
    }
}

/// A remote object read by byte ranges.
pub struct RemoteSource {
    reader: Reader<HttpHost>,
    /// Why the last read failed, for the error message: the `Source` trait
    /// carries only an I/O error kind.
    failure: Option<String>,
}

impl RemoteSource {
    /// Why the last read failed, if one did.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// The object's size, known since the first response.
    pub fn len_known(&self) -> u64 {
        self.reader.size()
    }

    /// What the reader did.
    pub fn stats(&self) -> coreav_range::Stats {
        self.reader.stats()
    }

    /// The failure of an operation on this source: the remote reason when a
    /// read failed, else `failure` as it is.
    pub fn explain(&self, failure: Failure) -> Failure {
        match (&failure, self.failure()) {
            (f, Some(reason)) if f.is_io() => Failure::Remote(reason.to_owned()),
            _ => failure,
        }
    }
}

impl Drop for RemoteSource {
    /// With `COREAV_REMOTE_STATS=1`, the reader's requests and bytes on
    /// standard error, for measurements.
    fn drop(&mut self) {
        if std::env::var_os("COREAV_REMOTE_STATS").is_some_and(|v| v == "1") {
            let s = self.reader.stats();
            let _ = writeln!(
                std::io::stderr(),
                "{{\"remote_stats\":{{\"requests\":{},\"pipelines\":{},\"bytes_requested\":{},\"bytes_received\":{},\"size\":{}}}}}",
                s.requests,
                s.pipelines,
                s.bytes_requested,
                s.bytes_received,
                self.reader.size()
            );
        }
    }
}

impl Source for RemoteSource {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.reader.size())
    }

    /// A writer's advice before its copy pass: the plan starts a pipeline
    /// there (`coreav_range::Reader::will_read`).
    fn will_read(&mut self, offset: u64, len: u64) {
        self.reader.will_read(offset, len);
    }

    /// A copy pass's or a scan's advice of the regions it reads: the plan
    /// requests them and skips the gaps between them
    /// (`coreav_range::Reader::will_read_regions`).
    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        self.reader.will_read_regions(regions);
    }

    fn wants_regions(&mut self) -> bool {
        true
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        if std::env::var_os("COREAV_REMOTE_TRACE").is_some() {
            let _ = writeln!(
                std::io::stderr(),
                "read {offset} {} requests={}",
                buf.len(),
                self.reader.stats().requests
            );
        }
        match self.reader.read(offset, buf) {
            Ok(()) => Ok(()),
            Err(coreav_range::Error::OutOfRange { .. }) => Err(Error::Truncated { offset }),
            Err(error) => {
                let kind = match error {
                    coreav_range::Error::NotFound(_) => std::io::ErrorKind::NotFound,
                    coreav_range::Error::Changed(_) => std::io::ErrorKind::InvalidData,
                    _ => std::io::ErrorKind::Other,
                };
                self.failure = Some(error.to_string());
                Err(Error::Io(kind))
            }
        }
    }
}

/// Bytes of the object already received, kept in an unnamed temporary file
/// (created readable by its owner only, and on Unix unlinked at once, so it
/// never outlives the process). Best effort: a failed write turns it off.
struct Spool {
    file: std::fs::File,
    #[cfg(not(unix))]
    path: std::path::PathBuf,
    /// The byte ranges held, sorted, disjoint and not touching.
    held: Vec<(u64, u64)>,
    written: u64,
    cap: u64,
}

impl Spool {
    fn create(cap: u64) -> Option<Self> {
        use std::hash::{BuildHasher, Hasher};
        for _ in 0..8 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            let path = std::env::temp_dir().join(format!("coreav-spool-{:016x}", hasher.finish()));
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            match options.open(&path) {
                Ok(file) => {
                    #[cfg(unix)]
                    {
                        let _ = std::fs::remove_file(&path);
                        return Some(Self {
                            file,
                            held: Vec::new(),
                            written: 0,
                            cap,
                        });
                    }
                    #[cfg(not(unix))]
                    {
                        return Some(Self {
                            file,
                            path,
                            held: Vec::new(),
                            written: 0,
                            cap,
                        });
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return None,
            }
        }
        None
    }

    /// Fills `buf` from the spool if it holds every byte.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        let end = offset.saturating_add(buf.len() as u64);
        if !self.held.iter().any(|(a, b)| *a <= offset && end <= *b) {
            return false;
        }
        read_at(&self.file, offset, buf).is_ok()
    }

    /// Keeps the bytes of a read, while the spool is under its cap.
    fn keep(&mut self, offset: u64, data: &[u8]) {
        let length = data.len() as u64;
        if length == 0 || self.written.saturating_add(length) > self.cap {
            return;
        }
        if write_at(&self.file, offset, data).is_err() {
            self.cap = 0;
            self.held.clear();
            return;
        }
        self.written = self.written.saturating_add(length);
        let (mut start, mut end) = (offset, offset.saturating_add(length));
        // Merge with every range it overlaps or touches.
        self.held.retain(|(a, b)| {
            if *b < start || *a > end {
                true
            } else {
                start = start.min(*a);
                end = end.max(*b);
                false
            }
        });
        let at = self.held.partition_point(|(a, _)| *a < start);
        self.held.insert(at, (start, end));
    }
}

#[cfg(not(unix))]
impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn read_at(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(unix)]
fn write_at(file: &std::fs::File, offset: u64, data: &[u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, data, offset)
}

#[cfg(not(unix))]
fn read_at(mut file: &std::fs::File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}

#[cfg(not(unix))]
fn write_at(mut file: &std::fs::File, offset: u64, data: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(data)
}

/// Opens `url`: checks it, makes the first request (which gives the
/// object's size) and returns the source.
pub fn open(url: &str, options: &Options) -> Result<RemoteSource, Failure> {
    let parsed = check_url(url, options).map_err(Failure::Remote)?;
    let host = HttpHost::new(parsed, options);
    let reader = Reader::open(host, options.settings.clone())
        .map_err(|error| Failure::Remote(error.to_string()))?;
    Ok(RemoteSource {
        reader,
        failure: None,
    })
}

/// Why `url` may not be read under `options`, if it may not (its scheme,
/// host and credentials; addresses are checked when connecting).
pub fn check(url: &str, options: &Options) -> Result<(), String> {
    check_url(url, options).map(|_| ())
}

/// The URL if it may be read: `https://` (or `http://` when allowed), a
/// host, no credentials.
fn check_url(url: &str, options: &Options) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("not a URL: {e}"))?;
    match parsed.scheme() {
        "https" => {}
        "http" if options.plain_http => {}
        "http" => return Err("only https:// URLs are read here".to_owned()),
        other => return Err(format!("{other}:// URLs are not read; use https://")),
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("the URL has no host".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("credentials in a URL are not accepted; use a presigned URL".to_owned());
    }
    Ok(parsed)
}

/// Whether `ip` is a public unicast address: not loopback, private,
/// link-local, shared (carrier-grade NAT), unspecified, broadcast,
/// multicast, documentation, benchmarking or reserved; for IPv6 also not
/// unique-local, and IPv4-mapped, IPv4-compatible and NAT64 addresses are
/// judged by their IPv4 address.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 88 && c == 99)
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            let [s0, s1, s2, s3, s4, s5, ..] = v6.segments();
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            // NAT64 (64:ff9b::/96) and the deprecated IPv4-compatible form
            // (::a.b.c.d) carry an IPv4 address in their last 32 bits.
            let nat64 = s0 == 0x64 && s1 == 0xff9b && s2 == 0 && s3 == 0 && s4 == 0 && s5 == 0;
            let compatible = s0 == 0 && s1 == 0 && s2 == 0 && s3 == 0 && s4 == 0 && s5 == 0;
            if nat64 || (compatible && !v6.is_loopback() && !v6.is_unspecified()) {
                let [.., a, b, c, d] = v6.octets();
                return is_public(IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d)));
            }
            // 6to4 (2002::/16) carries an IPv4 address in bits 16 to 48.
            if s0 == 0x2002 {
                let [_, _, a, b, c, d, ..] = v6.octets();
                return is_public(IpAddr::V4(std::net::Ipv4Addr::new(a, b, c, d)));
            }
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (s0 == 0x64 && s1 == 0xff9b && s2 == 1)
                || (s0 & 0xffc0) == 0xfec0
                || (s0 & 0xfe00) == 0xfc00
                || (s0 & 0xffc0) == 0xfe80
                || (s0 == 0x2001 && s1 == 0x0db8)
                || (s0 == 0x2001 && s1 < 0x0200))
        }
    }
}

/// The reason a guarded request was refused.
#[derive(Debug)]
struct Blocked(String);

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Blocked {}

/// The system resolver, with every answer checked when private addresses
/// are not allowed. The connection is made to the addresses returned here,
/// so the check covers it.
#[derive(Debug)]
struct Guard {
    private_network: bool,
}

impl Resolver for Guard {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addresses = DefaultResolver::default().resolve(uri, config, timeout)?;
        if !self.private_network {
            if let Some(refused) = addresses.iter().find(|a: &&SocketAddr| !is_public(a.ip())) {
                return Err(ureq::Error::Other(Box::new(Blocked(format!(
                    "{} resolves to {}, which is not a public address; this server reads public URLs only",
                    uri.host().unwrap_or("the host"),
                    refused.ip()
                )))));
            }
        }
        Ok(addresses)
    }
}

/// Makes the requests, each on its own thread.
struct HttpHost {
    agent: ureq::Agent,
    url: url::Url,
    idle: Duration,
    spool: Option<Spool>,
    /// When reading this object must stop, if ever.
    deadline: Option<std::time::Instant>,
}

impl HttpHost {
    fn new(url: url::Url, options: &Options) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::Rustls)
            .root_certs(ureq::tls::RootCerts::Specific(Arc::new(system_roots())))
            .unversioned_rustls_crypto_provider(provider)
            .build();
        let mut config = ureq::Agent::config_builder()
            .tls_config(tls)
            .max_redirects(0)
            .http_status_as_error(false)
            .accept_encoding(ureq::config::AutoHeaderValue::None)
            .user_agent(concat!("coreav/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(options.connect_timeout))
            .timeout_recv_response(Some(options.idle_timeout))
            .timeout_recv_body(Some(options.body_timeout));
        if !options.proxy_from_env {
            config = config.proxy(None);
        }
        let agent = ureq::Agent::with_parts(
            config.build(),
            DefaultConnector::default(),
            Guard {
                private_network: options.private_network,
            },
        );
        Self {
            agent,
            url,
            idle: options.idle_timeout,
            spool: (options.spool_bytes > 0)
                .then(|| Spool::create(options.spool_bytes))
                .flatten(),
            deadline: options
                .deadline
                .and_then(|d| std::time::Instant::now().checked_add(d)),
        }
    }
}

/// The operating system's trusted root certificates (the files OpenSSL
/// reads on Linux, the keychain on macOS); certificates that cannot be
/// read are left out.
fn system_roots() -> Vec<ureq::tls::Certificate<'static>> {
    rustls_native_certs::load_native_certs()
        .certs
        .iter()
        .map(|der| ureq::tls::Certificate::from_der(der.as_ref()).to_owned())
        .collect()
}

/// One request in flight: its head, then its body in chunks, from its
/// thread.
struct Handle {
    head: Option<Receiver<Result<Response, HostError>>>,
    chunks: Receiver<Result<Vec<u8>, HostError>>,
    chunk: Vec<u8>,
    at: usize,
    stop: Arc<AtomicBool>,
}

impl HttpHost {
    /// Fails once the object's reading time is up.
    fn within_deadline(&self) -> Result<(), HostError> {
        match self.deadline {
            Some(deadline) if std::time::Instant::now() > deadline => Err(HostError::Refused(
                "reading the object took longer than allowed".to_owned(),
            )),
            _ => Ok(()),
        }
    }
}

impl Host for HttpHost {
    type Handle = Handle;

    fn start(&mut self, request: &Request<'_>) -> Handle {
        let (head_tx, head_rx) = sync_channel(1);
        let depth = usize::try_from(request.read_ahead / CHUNK as u64)
            .unwrap_or(1)
            .max(1);
        let (chunks_tx, chunks_rx) = sync_channel(depth);
        let stop = Arc::new(AtomicBool::new(false));
        let job = Job {
            agent: self.agent.clone(),
            url: self.url.clone(),
            offset: request.offset,
            length: request.length,
            suffix: request.suffix,
            validator: request.validator.map(str::to_owned),
            stop: Arc::clone(&stop),
        };
        let started = std::thread::Builder::new()
            .name("coreav-range".to_owned())
            .spawn({
                let head_tx = head_tx.clone();
                move || job.run(&head_tx, &chunks_tx)
            });
        if let Err(error) = started {
            let _ = head_tx.send(Err(HostError::Retry(format!(
                "cannot start a request: {error}"
            ))));
        }
        Handle {
            head: Some(head_rx),
            chunks: chunks_rx,
            chunk: Vec::new(),
            at: 0,
            stop,
        }
    }

    fn response(&mut self, handle: &mut Handle) -> Result<Response, HostError> {
        self.within_deadline()?;
        let Some(head) = handle.head.take() else {
            return Err(HostError::Refused(
                "a response was asked for twice".to_owned(),
            ));
        };
        match head.recv_timeout(self.idle) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(HostError::Retry(format!(
                "no response within {} seconds",
                self.idle.as_secs()
            ))),
            Err(RecvTimeoutError::Disconnected) => Err(HostError::Retry(
                "the request ended without a response".to_owned(),
            )),
        }
    }

    fn read(&mut self, handle: &mut Handle, buf: &mut [u8]) -> Result<usize, HostError> {
        if handle.at >= handle.chunk.len() {
            self.within_deadline()?;
            match handle.chunks.recv_timeout(self.idle) {
                Ok(Ok(chunk)) => {
                    handle.chunk = chunk;
                    handle.at = 0;
                }
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(HostError::Retry(format!(
                        "no bytes within {} seconds",
                        self.idle.as_secs()
                    )));
                }
            }
        }
        let available = handle.chunk.get(handle.at..).unwrap_or_default();
        let n = available.len().min(buf.len());
        if let (Some(target), Some(source)) = (buf.get_mut(..n), available.get(..n)) {
            target.copy_from_slice(source);
        }
        handle.at = handle.at.saturating_add(n);
        Ok(n)
    }

    fn cancel(&mut self, handle: Handle) {
        handle.stop.store(true, Ordering::Relaxed);
        // Dropping the receivers ends the thread at its next send.
    }

    fn keep(&mut self, offset: u64, bytes: &[u8]) {
        if let Some(spool) = &mut self.spool {
            spool.keep(offset, bytes);
        }
    }

    fn kept(&mut self, offset: u64, buf: &mut [u8]) -> bool {
        self.spool
            .as_mut()
            .is_some_and(|spool| spool.read(offset, buf))
    }
}

/// One request, run on its thread.
struct Job {
    agent: ureq::Agent,
    url: url::Url,
    offset: u64,
    length: u64,
    suffix: bool,
    validator: Option<String>,
    stop: Arc<AtomicBool>,
}

impl Job {
    fn run(
        self,
        head: &SyncSender<Result<Response, HostError>>,
        chunks: &SyncSender<Result<Vec<u8>, HostError>>,
    ) {
        let started = std::time::Instant::now();
        let (response, body) = match self.fetch() {
            Ok((response, body)) => (
                Response {
                    wait: Some(started.elapsed()),
                    ..response
                },
                body,
            ),
            Err(error) => {
                let _ = head.send(Err(error));
                return;
            }
        };
        let left = response
            .end
            .unwrap_or(response.size)
            .saturating_sub(response.offset);
        if head.send(Ok(response)).is_err() {
            return;
        }
        let mut reader = body.into_reader().take(left);
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let mut chunk = vec![0u8; CHUNK];
            match reader.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => {
                    chunk.truncate(n);
                    // Exact size: a server sending a byte at a time cannot
                    // make each queued chunk hold a whole buffer.
                    chunk.shrink_to_fit();
                    if chunks.send(Ok(chunk)).is_err() {
                        return;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    let _ = chunks.send(Err(HostError::Retry(format!(
                        "reading a response: {error}"
                    ))));
                    return;
                }
            }
        }
    }

    /// The response to the range request, after the redirects it may
    /// follow, with its head checked.
    fn fetch(&self) -> Result<(Response, ureq::Body), HostError> {
        let mut url = self.url.clone();
        for _ in 0..=MAX_REDIRECTS {
            let range = if self.suffix {
                format!("bytes=-{}", self.length.max(1))
            } else {
                let last = self
                    .offset
                    .saturating_add(self.length.max(1))
                    .saturating_sub(1);
                format!("bytes={}-{last}", self.offset)
            };
            let mut request = self.agent.get(url.as_str()).header("range", range);
            if let Some(validator) = &self.validator {
                request = request.header("if-match", validator);
            }
            let response = request.call().map_err(describe)?;
            let status = response.status().as_u16();
            if (300..400).contains(&status) {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        HostError::Refused(format!("HTTP {status} without a Location"))
                    })?;
                let next = url.join(location).map_err(|e| {
                    HostError::Refused(format!("a redirect to an invalid URL: {e}"))
                })?;
                if next.scheme() != "https" {
                    return Err(HostError::Refused(format!(
                        "a redirect to {}:// (only https:// redirects are followed)",
                        next.scheme()
                    )));
                }
                url = next;
                continue;
            }
            return self.head(status, response);
        }
        Err(HostError::Refused(format!(
            "more than {MAX_REDIRECTS} redirects"
        )))
    }

    fn head(
        &self,
        status: u16,
        response: ureq::http::Response<ureq::Body>,
    ) -> Result<(Response, ureq::Body), HostError> {
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let etag = header("etag").filter(|e| !e.starts_with("W/"));
        match status {
            206 => {
                let range = header("content-range").unwrap_or_default();
                let (start, end, size) = content_range(&range).ok_or_else(|| {
                    HostError::Refused(format!("a 206 response with Content-Range {range:?}"))
                })?;
                let head = Response {
                    offset: start,
                    end: Some(end),
                    size,
                    validator: etag,
                    whole: false,
                    wait: None,
                };
                Ok((head, response.into_body()))
            }
            200 if self.offset == 0 => {
                // The server ignores ranges: the whole object from byte 0.
                let size = header("content-length")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .ok_or_else(|| {
                        HostError::Refused("a 200 response without a Content-Length".to_owned())
                    })?;
                let head = Response {
                    offset: 0,
                    end: Some(size),
                    size,
                    validator: etag,
                    whole: true,
                    wait: None,
                };
                Ok((head, response.into_body()))
            }
            416 if self.offset == 0 => Ok((
                Response {
                    offset: 0,
                    end: Some(0),
                    size: 0,
                    validator: etag,
                    whole: false,
                    wait: None,
                },
                response.into_body(),
            )),
            200 => Err(HostError::Refused(
                "the server does not answer range requests".to_owned(),
            )),
            412 => Err(HostError::Changed(
                "the object changed (If-Match failed)".to_owned(),
            )),
            404 | 410 => Err(HostError::NotFound(format!("HTTP {status}"))),
            408 | 425 | 429 | 500..=599 => Err(HostError::Retry(format!("HTTP {status}"))),
            _ => Err(HostError::Refused(format!(
                "HTTP {status} for a range request"
            ))),
        }
    }
}

/// `bytes FIRST-LAST/SIZE` as `(FIRST, LAST + 1, SIZE)`.
fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, size) = rest.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let first: u64 = first.trim().parse().ok()?;
    let last: u64 = last.trim().parse().ok()?;
    let size: u64 = size.trim().parse().ok()?;
    (first <= last && last < size).then(|| (first, last.saturating_add(1), size))
}

/// A request's failure as the plan sees it: retried unless a new request
/// cannot fix it.
fn describe(error: ureq::Error) -> HostError {
    match error {
        ureq::Error::Other(inner) if inner.is::<Blocked>() => HostError::Refused(inner.to_string()),
        ureq::Error::BadUri(m) => HostError::Refused(format!("invalid URL: {m}")),
        ureq::Error::HostNotFound => {
            HostError::Refused("the host name does not resolve".to_owned())
        }
        ureq::Error::Tls(m) => HostError::Refused(format!("TLS: {m}")),
        ureq::Error::Rustls(e) => HostError::Refused(format!("TLS: {e}")),
        other => HostError::Retry(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_addresses() {
        for public in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "::7f00:1",
            "2002:7f00:1::1",
            "2002:c0a8:101::",
            "64:ff9b:1::a00:1",
            "fec0::1",
            "192.88.99.1",
        ] {
            assert!(!is_public(private.parse().unwrap()), "{private}");
        }
        assert!(is_public("::ffff:8.8.8.8".parse().unwrap()));
        assert!(is_public("64:ff9b::808:808".parse().unwrap()));
        assert!(is_public("2002:808:808::1".parse().unwrap()));
    }

    #[test]
    fn the_guarded_mode_reads_public_https_urls_only() {
        let refusal = |url: &str| match open(url, &Options::guarded()) {
            Err(Failure::Remote(reason)) => reason,
            _ => panic!("{url} was not refused as a remote failure"),
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let reason = refusal(&format!("http://127.0.0.1:{port}/a.mp4"));
        assert!(reason.contains("only https://"), "{reason}");
        let reason = refusal(&format!("https://127.0.0.1:{port}/a.mp4"));
        assert!(reason.contains("not a public address"), "{reason}");
        assert_eq!(
            listener.accept().map(|_| ()).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "a refused address was connected to"
        );
        let reason = refusal("https://user:secret@example.com/a.mp4");
        assert!(reason.contains("credentials"), "{reason}");
    }

    #[test]
    fn urls_and_content_ranges() {
        assert!(is_url("https://example.com/a.mp4"));
        assert!(is_url("HTTP://example.com/a.mp4"));
        assert!(is_url("s3://bucket/key"));
        assert!(!is_url("/tmp/a.mp4"));
        assert!(!is_url("a:b.mp4"));
        assert_eq!(content_range("bytes 0-99/1000"), Some((0, 100, 1000)));
        assert_eq!(content_range("bytes 0-99/*"), None);
        assert_eq!(content_range("bytes 5-4/10"), None);
        assert_eq!(content_range("bytes 0-10/10"), None);
    }
}
