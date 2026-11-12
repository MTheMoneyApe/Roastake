//! Generic fail-closed HTTP transport abstraction and WASI Preview 2 client.
//!
//! Designed for two targets:
//! - Host unit tests (`cargo test`): exercises `MockHttpTransport` and every URL,
//!   allowlist, deadline, response-size, and redaction rule with zero network I/O.
//! - WASI Preview 2 component (`wasm32-wasip2 --features wasi-http`): executes real
//!   HTTPS calls strictly through `wasi:http/outgoing-handler` using explicit
//!   `RequestOptions` deadlines (3s connect / 8s first-byte / 8s between-bytes).
//!
//! Note: Local URL and hostname checks are defense-in-depth and do NOT replace
//! OutLayer's host-side DNS/SSRF guard or embedded `outlayer.manifest` policy.

#![allow(dead_code)]

use core::fmt;
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::time::Duration;
use url::{Host, Url};

/// Embedded `outlayer.manifest` bytes compiled directly into the WASM custom section.
pub const EMBEDDED_OUTLAYER_MANIFEST_JSON: &[u8] = include_bytes!("../outlayer.manifest.json");

/// Maximum connect deadline enforced on every outgoing HTTP request (3 seconds).
pub const CONNECT_TIMEOUT_SECS: u64 = 3;

/// Maximum wait for initial response status/headers after connect (8 seconds).
pub const FIRST_BYTE_TIMEOUT_SECS: u64 = 8;

/// Maximum pause between incoming body chunks (8 seconds).
pub const BETWEEN_BYTES_TIMEOUT_SECS: u64 = 8;

/// Maximum buffered HTTP response body size (512 KiB = 524,288 bytes).
pub const MAX_HTTP_RESPONSE_BYTES: usize = 512 * 1024;

/// Maximum chunk requested per WASI stream read iteration (64 KiB).
const STREAM_READ_CHUNK_BYTES: usize = 64 * 1024;

/// Maximum header length check to prevent huge control-plane payloads.
const MAX_HEADER_VALUE_BYTES: usize = 4096;

/// Outgoing request deadlines configured on every WASI HTTP call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpDeadlines {
    pub connect_timeout: Duration,
    pub first_byte_timeout: Duration,
    pub between_bytes_timeout: Duration,
}

impl Default for HttpDeadlines {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(CONNECT_TIMEOUT_SECS),
            first_byte_timeout: Duration::from_secs(FIRST_BYTE_TIMEOUT_SECS),
            between_bytes_timeout: Duration::from_secs(BETWEEN_BYTES_TIMEOUT_SECS),
        }
    }
}

/// Timeout phase reported cleanly without leaking URLs or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutPhase {
    Connect,
    FirstByte,
    BetweenBytes,
    Unknown,
}

/// Closed, secret-redacting transport error enum.
///
/// Structurally omits raw URLs, query parameters, authorization headers,
/// and response body fragments so secrets can never leak into `Display` or `Debug`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    NotConfigured {
        provider: Option<&'static str>,
    },
    InvalidUrl {
        provider: Option<&'static str>,
        reason: &'static str,
    },
    UnsupportedScheme {
        provider: Option<&'static str>,
    },
    HostNotAllowed {
        provider: Option<&'static str>,
    },
    InvalidManifest {
        reason: &'static str,
    },
    Timeout {
        provider: Option<&'static str>,
        phase: TimeoutPhase,
    },
    TimeoutConfigError {
        provider: Option<&'static str>,
        field: &'static str,
    },
    ConnectError {
        provider: Option<&'static str>,
    },
    HttpStatus {
        provider: Option<&'static str>,
        status: u16,
    },
    ResponseTooLarge {
        provider: Option<&'static str>,
        limit_bytes: usize,
    },
    MalformedResponse {
        provider: Option<&'static str>,
        reason: &'static str,
    },
    StreamReadError {
        provider: Option<&'static str>,
    },
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured { provider } => write_with_provider(f, *provider, "not configured"),
            Self::InvalidUrl { provider, reason } => {
                write_with_provider(f, *provider, "invalid request URL: ")?;
                f.write_str(reason)
            }
            Self::UnsupportedScheme { provider } => write_with_provider(
                f,
                *provider,
                "unsupported URL scheme (only https permitted)",
            ),
            Self::HostNotAllowed { provider } => {
                write_with_provider(f, *provider, "destination host is not in allowlist")
            }
            Self::InvalidManifest { reason } => {
                write!(f, "invalid embedded outlayer.manifest: {}", reason)
            }
            Self::Timeout { provider, phase } => {
                write_with_provider(f, *provider, "request timed out during ")?;
                match phase {
                    TimeoutPhase::Connect => f.write_str("connect"),
                    TimeoutPhase::FirstByte => f.write_str("first-byte wait"),
                    TimeoutPhase::BetweenBytes => f.write_str("stream read"),
                    TimeoutPhase::Unknown => f.write_str("request"),
                }
            }
            Self::TimeoutConfigError { provider, field } => {
                write_with_provider(f, *provider, "failed to configure WASI timeout option: ")?;
                f.write_str(field)
            }
            Self::ConnectError { provider } => {
                write_with_provider(f, *provider, "network connection failed")
            }
            Self::HttpStatus { provider, status } => {
                write_with_provider(f, *provider, "unexpected HTTP status ")?;
                write!(f, "{}", status)
            }
            Self::ResponseTooLarge {
                provider,
                limit_bytes,
            } => {
                write_with_provider(f, *provider, "response exceeded max ")?;
                write!(f, "{} bytes", limit_bytes)
            }
            Self::MalformedResponse { provider, reason } => {
                write_with_provider(f, *provider, "malformed response: ")?;
                f.write_str(reason)
            }
            Self::StreamReadError { provider } => {
                write_with_provider(f, *provider, "response body stream read failed")
            }
        }
    }
}

impl std::error::Error for TransportError {}

fn write_with_provider(
    f: &mut fmt::Formatter<'_>,
    provider: Option<&'static str>,
    msg: &str,
) -> fmt::Result {
    match provider {
        Some(id) => write!(f, "transport error [{}]: {}", id, msg),
        None => write!(f, "transport error: {}", msg),
    }
}

/// Supported HTTP methods for oracle requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

/// Redacted HTTP request specification.
///
/// Custom `Debug` omits raw URL query strings, headers, and bodies so secrets
/// cannot be printed by diagnostic formatters.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub provider_id: Option<&'static str>,
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub deadlines: HttpDeadlines,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let host_only = Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "<unparsed>".to_string());
        f.debug_struct("HttpRequest")
            .field("provider_id", &self.provider_id)
            .field("method", &self.method)
            .field("host", &host_only)
            .field("header_count", &self.headers.len())
            .field("body_bytes", &self.body.as_ref().map_or(0, Vec::len))
            .field("deadlines", &self.deadlines)
            .finish()
    }
}

impl HttpRequest {
    pub fn get(provider_id: Option<&'static str>, url: impl Into<String>) -> Self {
        Self {
            provider_id,
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: None,
            deadlines: HttpDeadlines::default(),
        }
    }

    pub fn post(
        provider_id: Option<&'static str>,
        url: impl Into<String>,
        body: Option<Vec<u8>>,
    ) -> Self {
        Self {
            provider_id,
            method: HttpMethod::Post,
            url: url.into(),
            headers: Vec::new(),
            body,
            deadlines: HttpDeadlines::default(),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let key = name.into();
        let val = value.into();
        self.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(&key));
        self.headers.push((key, val));
        self
    }

    pub fn with_deadlines(mut self, deadlines: HttpDeadlines) -> Self {
        self.deadlines = deadlines;
        self
    }
}

/// Bounded HTTP response captured from transport execution.
///
/// Custom `Debug` omits raw body bytes so tokens echoed by upstreams cannot leak.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

impl HttpResponse {
    /// Parse response body as typed JSON, suppressing raw payload fragments on failure.
    pub fn json<T: DeserializeOwned>(
        &self,
        provider: Option<&'static str>,
    ) -> Result<T, TransportError> {
        serde_json::from_slice(&self.body).map_err(|_| TransportError::MalformedResponse {
            provider,
            reason: "failed to parse JSON response body",
        })
    }
}

/// Exact hostname allowlist (case-insensitive, no implicit subdomain wildcards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAllowlist {
    allowed_hosts: BTreeSet<String>,
}

impl NetworkAllowlist {
    /// Create a fail-closed empty allowlist blocking all hosts.
    pub fn empty() -> Self {
        Self {
            allowed_hosts: BTreeSet::new(),
        }
    }

    /// Parse the embedded `outlayer.manifest` bytes and construct a matching local allowlist.
    ///
    /// Strictly requires:
    /// - Valid JSON object with `capabilities.network` present as a JSON array (not missing or `null`).
    /// - Absence of `connector_id`, `author_secrets`, and `operations`.
    /// - Exact domain hostnames only (no wildcards `*`, no IP literals).
    pub fn from_embedded_manifest() -> Result<Self, TransportError> {
        Self::from_manifest_bytes(EMBEDDED_OUTLAYER_MANIFEST_JSON)
    }

    pub fn from_manifest_bytes(bytes: &[u8]) -> Result<Self, TransportError> {
        let root: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| TransportError::InvalidManifest {
                reason: "manifest is not valid JSON",
            })?;
        let obj = root.as_object().ok_or(TransportError::InvalidManifest {
            reason: "manifest root must be a JSON object",
        })?;

        for forbidden in ["connector_id", "author_secrets", "operations", "limits"] {
            if obj.contains_key(forbidden) {
                return Err(TransportError::InvalidManifest {
                    reason: "forbidden connector or author_secrets member present in manifest",
                });
            }
        }

        let capabilities = obj.get("capabilities").and_then(|v| v.as_object()).ok_or(
            TransportError::InvalidManifest {
                reason: "manifest must declare capabilities object",
            },
        )?;

        let network = capabilities
            .get("network")
            .and_then(|v| v.as_array())
            .ok_or(TransportError::InvalidManifest {
                reason: "capabilities.network must be an explicit JSON array",
            })?;

        let mut hosts = Vec::with_capacity(network.len());
        for entry in network {
            let host = entry.as_str().ok_or(TransportError::InvalidManifest {
                reason: "network allowlist entries must be host strings",
            })?;
            hosts.push(host);
        }

        Self::from_hosts(&hosts)
    }

    /// Construct an allowlist from explicit hostname entries, rejecting wildcards or IPs.
    pub fn from_hosts(hosts: &[&str]) -> Result<Self, TransportError> {
        let mut allowed_hosts = BTreeSet::new();
        for raw in hosts {
            let normalized = validate_domain_hostname(raw, None)?;
            allowed_hosts.insert(normalized);
        }
        Ok(Self { allowed_hosts })
    }

    pub fn is_empty(&self) -> bool {
        self.allowed_hosts.is_empty()
    }

    pub fn len(&self) -> usize {
        self.allowed_hosts.len()
    }

    /// Exact case-insensitive host match with trailing DNS root dot normalized; no wildcards.
    pub fn allows(&self, host: &str) -> bool {
        let Ok(normalized) = normalize_valid_host_chars(host) else {
            return false;
        };
        self.allowed_hosts.contains(&normalized)
    }
}

/// Parsed and policy-checked HTTPS request target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedUrl {
    pub url: Url,
    pub normalized_host: String,
    pub path_with_query: String,
}

/// Validate that a request URL is strictly HTTPS, unambiguous, non-IP-literal,
/// credential-free, fragment-free, and permitted by the explicit allowlist.
pub fn validate_request_url(
    raw_url: &str,
    allowlist: &NetworkAllowlist,
    provider: Option<&'static str>,
) -> Result<ValidatedUrl, TransportError> {
    if raw_url.is_empty()
        || raw_url
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err(TransportError::InvalidUrl {
            provider,
            reason: "URL cannot be empty or contain whitespace/control bytes",
        });
    }

    let Some((scheme_part, after_scheme)) = raw_url.split_once("://") else {
        return Err(TransportError::InvalidUrl {
            provider,
            reason: "URL must be absolute with :// delimiter",
        });
    };

    if !scheme_part.eq_ignore_ascii_case("https") {
        return Err(TransportError::UnsupportedScheme { provider });
    }

    // Inspect raw authority segment before path/query/fragment to reject ambiguous encodings.
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let raw_authority = &after_scheme[..authority_end];
    if raw_authority.is_empty() {
        return Err(TransportError::InvalidUrl {
            provider,
            reason: "URL authority cannot be empty",
        });
    }
    if raw_authority.contains(['@', '\\']) {
        return Err(TransportError::InvalidUrl {
            provider,
            reason: "embedded credentials or backslashes in authority are rejected",
        });
    }

    let parsed = Url::parse(raw_url).map_err(|_| TransportError::InvalidUrl {
        provider,
        reason: "URL failed strict parser validation",
    })?;

    if parsed.scheme() != "https" {
        return Err(TransportError::UnsupportedScheme { provider });
    }
    if parsed.cannot_be_a_base() {
        return Err(TransportError::InvalidUrl {
            provider,
            reason: "URL must have hierarchical authority/path",
        });
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(TransportError::InvalidUrl {
            provider,
            reason: "embedded credentials are not allowed",
        });
    }
    if parsed.fragment().is_some() {
        return Err(TransportError::InvalidUrl {
            provider,
            reason: "URL fragments are not allowed",
        });
    }

    let normalized_host = match parsed.host() {
        None => {
            return Err(TransportError::InvalidUrl {
                provider,
                reason: "URL host is missing",
            })
        }
        Some(Host::Ipv4(_)) | Some(Host::Ipv6(_)) => {
            return Err(TransportError::InvalidUrl {
                provider,
                reason: "IP-literal targets are not allowed",
            })
        }
        Some(Host::Domain(domain)) => validate_domain_hostname(domain, provider)?,
    };

    if !allowlist.allows(&normalized_host) {
        return Err(TransportError::HostNotAllowed { provider });
    }

    let path_with_query = match parsed.query() {
        Some(q) => format!("{}?{}", parsed.path(), q),
        None => parsed.path().to_string(),
    };

    Ok(ValidatedUrl {
        url: parsed,
        normalized_host,
        path_with_query,
    })
}

fn normalize_valid_host_chars(raw: &str) -> Result<String, ()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains('*') || trimmed.contains("..") {
        return Err(());
    }
    let stripped = trimmed.strip_suffix('.').unwrap_or(trimmed);
    if stripped.is_empty() || stripped.ends_with('.') || stripped.starts_with('.') {
        return Err(());
    }
    Ok(stripped.to_ascii_lowercase())
}

fn validate_domain_hostname(
    raw_host: &str,
    provider: Option<&'static str>,
) -> Result<String, TransportError> {
    let normalized = normalize_valid_host_chars(raw_host).map_err(|()| {
        if let Some(_p) = provider {
            TransportError::InvalidUrl {
                provider,
                reason: "malformed or wildcard hostname",
            }
        } else {
            TransportError::InvalidManifest {
                reason: "malformed, empty, or wildcard hostname",
            }
        }
    })?;

    if normalized.parse::<IpAddr>().is_ok()
        || (normalized.starts_with('[') && normalized.ends_with(']'))
    {
        return Err(match provider {
            Some(_) => TransportError::InvalidUrl {
                provider,
                reason: "IP-literal hostname is not allowed",
            },
            None => TransportError::InvalidManifest {
                reason: "IP-literal entries are not allowed in allowlist",
            },
        });
    }

    for label in normalized.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(match provider {
                Some(_) => TransportError::InvalidUrl {
                    provider,
                    reason: "invalid DNS hostname label",
                },
                None => TransportError::InvalidManifest {
                    reason: "invalid DNS hostname label in allowlist",
                },
            });
        }
    }

    Ok(normalized)
}

/// Incrementally enforce checked byte capacity before buffering any response chunk.
pub fn checked_next_len(
    current_len: usize,
    chunk_len: usize,
    max_bytes: usize,
    provider: Option<&'static str>,
) -> Result<usize, TransportError> {
    let next = current_len
        .checked_add(chunk_len)
        .ok_or(TransportError::ResponseTooLarge {
            provider,
            limit_bytes: max_bytes,
        })?;
    if next > max_bytes {
        return Err(TransportError::ResponseTooLarge {
            provider,
            limit_bytes: max_bytes,
        });
    }
    Ok(next)
}

/// Append one chunk into `buf` only after verifying checked byte capacity.
pub fn append_bounded_chunk(
    buf: &mut Vec<u8>,
    chunk: &[u8],
    max_bytes: usize,
    provider: Option<&'static str>,
) -> Result<(), TransportError> {
    let _next = checked_next_len(buf.len(), chunk.len(), max_bytes, provider)?;
    buf.extend_from_slice(chunk);
    Ok(())
}

/// Stream chunks through bounded verification without buffering oversized payloads.
pub fn read_chunks_bounded<'a, I>(
    chunks: I,
    max_bytes: usize,
    provider: Option<&'static str>,
) -> Result<Vec<u8>, TransportError>
where
    I: IntoIterator<Item = &'a [u8]>,
{
    let mut buf = Vec::new();
    for chunk in chunks {
        append_bounded_chunk(&mut buf, chunk, max_bytes, provider)?;
    }
    Ok(buf)
}

/// WASI `RequestOptions` deadline abstraction enabling direct host unit testing of
/// setter invocation and setter-error propagation.
pub trait RequestDeadlinesSetter {
    fn set_connect_timeout_ns(&mut self, ns: u64) -> Result<(), ()>;
    fn set_first_byte_timeout_ns(&mut self, ns: u64) -> Result<(), ()>;
    fn set_between_bytes_timeout_ns(&mut self, ns: u64) -> Result<(), ()>;
}

/// Apply all three WASI HTTP deadlines (3s connect / 8s first-byte / 8s between-bytes).
///
/// Propagates every setter error; never silently ignores timeout configuration failures.
pub fn apply_http_deadlines<T: RequestDeadlinesSetter>(
    options: &mut T,
    deadlines: HttpDeadlines,
    provider: Option<&'static str>,
) -> Result<(), TransportError> {
    let connect_ns = duration_to_nanos(deadlines.connect_timeout, provider, "connect_timeout")?;
    let first_byte_ns =
        duration_to_nanos(deadlines.first_byte_timeout, provider, "first_byte_timeout")?;
    let between_bytes_ns = duration_to_nanos(
        deadlines.between_bytes_timeout,
        provider,
        "between_bytes_timeout",
    )?;

    options.set_connect_timeout_ns(connect_ns).map_err(|()| {
        TransportError::TimeoutConfigError {
            provider,
            field: "connect_timeout",
        }
    })?;
    options
        .set_first_byte_timeout_ns(first_byte_ns)
        .map_err(|()| TransportError::TimeoutConfigError {
            provider,
            field: "first_byte_timeout",
        })?;
    options
        .set_between_bytes_timeout_ns(between_bytes_ns)
        .map_err(|()| TransportError::TimeoutConfigError {
            provider,
            field: "between_bytes_timeout",
        })?;

    Ok(())
}

fn duration_to_nanos(
    duration: Duration,
    provider: Option<&'static str>,
    field: &'static str,
) -> Result<u64, TransportError> {
    if duration.is_zero() {
        return Err(TransportError::TimeoutConfigError { provider, field });
    }
    u64::try_from(duration.as_nanos())
        .map_err(|_| TransportError::TimeoutConfigError { provider, field })
}

/// Backend HTTP execution contract separated cleanly from URL policy and caller code.
pub trait HttpTransport {
    fn send_validated(
        &self,
        request: &HttpRequest,
        validated_url: &ValidatedUrl,
    ) -> Result<HttpResponse, TransportError>;
}

impl<T: HttpTransport> HttpTransport for &T {
    fn send_validated(
        &self,
        request: &HttpRequest,
        validated_url: &ValidatedUrl,
    ) -> Result<HttpResponse, TransportError> {
        (*self).send_validated(request, validated_url)
    }
}

/// Fail-closed policy-enforcing HTTP client wrapping any `HttpTransport`.
///
/// Enforces:
/// - Embedded/explicit `NetworkAllowlist` and strict HTTPS URL checks.
/// - No automatic retries (especially never retrying timeouts under OutLayer's 2-timeout kill rule).
/// - Strict bounded response reads (`<= MAX_HTTP_RESPONSE_BYTES`).
/// - Non-2xx status rejection without returning raw credential-bearing error pages.
pub struct HttpClient<T: HttpTransport> {
    transport: T,
    allowlist: NetworkAllowlist,
    max_response_bytes: usize,
}

impl<T: HttpTransport> HttpClient<T> {
    pub fn new(transport: T, allowlist: NetworkAllowlist) -> Self {
        Self {
            transport,
            allowlist,
            max_response_bytes: MAX_HTTP_RESPONSE_BYTES,
        }
    }

    pub fn with_max_response_bytes(mut self, max_bytes: usize) -> Self {
        self.max_response_bytes = max_bytes;
        self
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn allowlist(&self) -> &NetworkAllowlist {
        &self.allowlist
    }

    /// Validate URL, headers, and policy, execute exactly once (no retries), and check response bounds & status.
    pub fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let provider = request.provider_id;
        let validated = validate_request_url(&request.url, &self.allowlist, provider)?;
        validate_headers(&request.headers, provider)?;

        let response = self.transport.send_validated(request, &validated)?;
        if response.body.len() > self.max_response_bytes {
            return Err(TransportError::ResponseTooLarge {
                provider,
                limit_bytes: self.max_response_bytes,
            });
        }
        if !(200..=299).contains(&response.status) {
            return Err(TransportError::HttpStatus {
                provider,
                status: response.status,
            });
        }

        Ok(response)
    }
}

fn validate_headers(
    headers: &[(String, String)],
    provider: Option<&'static str>,
) -> Result<(), TransportError> {
    for (name, value) in headers {
        if name.is_empty()
            || name.len() > 256
            || name
                .bytes()
                .any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || b == b':')
        {
            return Err(TransportError::InvalidUrl {
                provider,
                reason: "invalid HTTP header name",
            });
        }
        if value.len() > MAX_HEADER_VALUE_BYTES
            || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
        {
            return Err(TransportError::InvalidUrl {
                provider,
                reason: "invalid HTTP header value",
            });
        }
    }
    Ok(())
}

/// Scripted mock HTTP response behavior for native tests.
#[derive(Debug, Clone)]
pub enum MockResponseAction {
    Success { status: u16, chunks: Vec<Vec<u8>> },
    Error(TransportError),
}

/// Safe audit record of executed mock requests (never captures query strings or headers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedMockCall {
    pub method: HttpMethod,
    pub host: String,
    pub path_only: String,
    pub deadlines: HttpDeadlines,
}

/// Native test double implementing `HttpTransport` with scripted responses and zero I/O.
#[derive(Debug, Default)]
pub struct MockHttpTransport {
    queue: std::cell::RefCell<std::collections::VecDeque<MockResponseAction>>,
    calls: std::cell::RefCell<Vec<RecordedMockCall>>,
    max_response_bytes: usize,
}

impl MockHttpTransport {
    pub fn new() -> Self {
        Self {
            queue: std::cell::RefCell::new(std::collections::VecDeque::new()),
            calls: std::cell::RefCell::new(Vec::new()),
            max_response_bytes: MAX_HTTP_RESPONSE_BYTES,
        }
    }

    pub fn with_max_response_bytes(mut self, max_bytes: usize) -> Self {
        self.max_response_bytes = max_bytes;
        self
    }

    pub fn enqueue(&self, action: MockResponseAction) {
        self.queue.borrow_mut().push_back(action);
    }

    pub fn enqueue_ok_json(&self, status: u16, body: impl Into<Vec<u8>>) {
        self.enqueue(MockResponseAction::Success {
            status,
            chunks: vec![body.into()],
        });
    }

    pub fn enqueue_ok_chunks(&self, status: u16, chunks: Vec<Vec<u8>>) {
        self.enqueue(MockResponseAction::Success { status, chunks });
    }

    pub fn enqueue_err(&self, err: TransportError) {
        self.enqueue(MockResponseAction::Error(err));
    }

    pub fn recorded_calls(&self) -> Vec<RecordedMockCall> {
        self.calls.borrow().clone()
    }
}

impl HttpTransport for MockHttpTransport {
    fn send_validated(
        &self,
        request: &HttpRequest,
        validated_url: &ValidatedUrl,
    ) -> Result<HttpResponse, TransportError> {
        self.calls.borrow_mut().push(RecordedMockCall {
            method: request.method,
            host: validated_url.normalized_host.clone(),
            path_only: validated_url.url.path().to_string(),
            deadlines: request.deadlines,
        });

        let action = self
            .queue
            .borrow_mut()
            .pop_front()
            .ok_or(TransportError::NotConfigured {
                provider: request.provider_id,
            })?;

        match action {
            MockResponseAction::Error(err) => Err(err),
            MockResponseAction::Success { status, chunks } => {
                let mut body = Vec::new();
                for chunk in &chunks {
                    append_bounded_chunk(
                        &mut body,
                        chunk,
                        self.max_response_bytes,
                        request.provider_id,
                    )?;
                }
                Ok(HttpResponse { status, body })
            }
        }
    }
}

/// Live WASI Preview 2 HTTP transport backed directly by `wasi::http::outgoing_handler`.
#[cfg(feature = "wasi-http")]
pub struct WasiHttpTransport;

#[cfg(feature = "wasi-http")]
impl RequestDeadlinesSetter for wasi::http::types::RequestOptions {
    fn set_connect_timeout_ns(&mut self, ns: u64) -> Result<(), ()> {
        self.set_connect_timeout(Some(ns))
    }

    fn set_first_byte_timeout_ns(&mut self, ns: u64) -> Result<(), ()> {
        self.set_first_byte_timeout(Some(ns))
    }

    fn set_between_bytes_timeout_ns(&mut self, ns: u64) -> Result<(), ()> {
        self.set_between_bytes_timeout(Some(ns))
    }
}

#[cfg(feature = "wasi-http")]
impl WasiHttpTransport {
    pub fn new() -> Self {
        Self
    }

    fn map_wasi_http_error(
        err: &wasi::http::types::ErrorCode,
        provider: Option<&'static str>,
        in_body_stream: bool,
    ) -> TransportError {
        use wasi::http::types::ErrorCode;
        match err {
            ErrorCode::ConnectionTimeout => TransportError::Timeout {
                provider,
                phase: TimeoutPhase::Connect,
            },
            ErrorCode::ConnectionReadTimeout => TransportError::Timeout {
                provider,
                phase: if in_body_stream {
                    TimeoutPhase::BetweenBytes
                } else {
                    TimeoutPhase::FirstByte
                },
            },
            ErrorCode::ConnectionWriteTimeout => TransportError::Timeout {
                provider,
                phase: TimeoutPhase::Unknown,
            },
            ErrorCode::HttpResponseBodySize(_) => TransportError::ResponseTooLarge {
                provider,
                limit_bytes: MAX_HTTP_RESPONSE_BYTES,
            },
            _ if in_body_stream => TransportError::StreamReadError { provider },
            _ => TransportError::ConnectError { provider },
        }
    }
}

#[cfg(feature = "wasi-http")]
impl Default for WasiHttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "wasi-http")]
impl HttpTransport for WasiHttpTransport {
    fn send_validated(
        &self,
        request: &HttpRequest,
        validated_url: &ValidatedUrl,
    ) -> Result<HttpResponse, TransportError> {
        use wasi::http::outgoing_handler;
        use wasi::http::types::{
            http_error_code, Fields, Method, OutgoingBody, OutgoingRequest, RequestOptions, Scheme,
        };
        use wasi::io::streams::StreamError;

        let provider = request.provider_id;
        let header_pairs: Vec<(String, Vec<u8>)> = request
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.as_bytes().to_vec()))
            .collect();
        let headers = Fields::from_list(&header_pairs).map_err(|_| TransportError::InvalidUrl {
            provider,
            reason: "failed to construct WASI HTTP headers",
        })?;

        let outgoing = OutgoingRequest::new(headers);
        let method = match request.method {
            HttpMethod::Get => Method::Get,
            HttpMethod::Post => Method::Post,
        };
        outgoing
            .set_method(&method)
            .map_err(|()| TransportError::InvalidUrl {
                provider,
                reason: "failed to set HTTP method",
            })?;
        outgoing
            .set_scheme(Some(&Scheme::Https))
            .map_err(|()| TransportError::UnsupportedScheme { provider })?;
        outgoing
            .set_authority(Some(validated_url.url.authority()))
            .map_err(|()| TransportError::InvalidUrl {
                provider,
                reason: "failed to set URL authority",
            })?;
        outgoing
            .set_path_with_query(Some(&validated_url.path_with_query))
            .map_err(|()| TransportError::InvalidUrl {
                provider,
                reason: "failed to set path and query",
            })?;

        let outgoing_body = outgoing
            .body()
            .map_err(|()| TransportError::ConnectError { provider })?;
        if let Some(body) = &request.body {
            let stream = outgoing_body
                .write()
                .map_err(|()| TransportError::ConnectError { provider })?;
            // WASI `blocking_write_and_flush` bounds individual writes to 4096 bytes.
            for chunk in body.chunks(4096) {
                stream
                    .blocking_write_and_flush(chunk)
                    .map_err(|_| TransportError::ConnectError { provider })?;
            }
        }
        OutgoingBody::finish(outgoing_body, None)
            .map_err(|_| TransportError::ConnectError { provider })?;

        let mut options = RequestOptions::new();
        apply_http_deadlines(&mut options, request.deadlines, provider)?;

        let future_resp = outgoing_handler::handle(outgoing, Some(options))
            .map_err(|e| Self::map_wasi_http_error(&e, provider, false))?;

        let incoming = match future_resp.get() {
            Some(result) => result
                .map_err(|()| TransportError::StreamReadError { provider })?
                .map_err(|e| Self::map_wasi_http_error(&e, provider, false))?,
            None => {
                future_resp.subscribe().block();
                future_resp
                    .get()
                    .ok_or(TransportError::StreamReadError { provider })?
                    .map_err(|()| TransportError::StreamReadError { provider })?
                    .map_err(|e| Self::map_wasi_http_error(&e, provider, false))?
            }
        };
        drop(future_resp);

        let status = incoming.status();
        let incoming_body = incoming
            .consume()
            .map_err(|()| TransportError::StreamReadError { provider })?;
        drop(incoming);

        let mut buf = Vec::new();
        {
            let stream = incoming_body
                .stream()
                .map_err(|()| TransportError::StreamReadError { provider })?;
            loop {
                // Probe at most `clamp(1, STREAM_READ_CHUNK_BYTES)` bytes so 512 KiB + 1 fails immediately without over-buffering.
                let read_cap = MAX_HTTP_RESPONSE_BYTES
                    .saturating_sub(buf.len())
                    .clamp(1, STREAM_READ_CHUNK_BYTES) as u64;
                match stream.blocking_read(read_cap) {
                    Ok(chunk) => {
                        append_bounded_chunk(&mut buf, &chunk, MAX_HTTP_RESPONSE_BYTES, provider)?;
                    }
                    Err(StreamError::Closed) => break,
                    Err(StreamError::LastOperationFailed(err)) => {
                        return Err(match http_error_code(&err) {
                            Some(code) => Self::map_wasi_http_error(&code, provider, true),
                            None => TransportError::StreamReadError { provider },
                        });
                    }
                }
            }
        }
        drop(incoming_body);

        Ok(HttpResponse { status, body: buf })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct SpyOptions {
        calls: Vec<&'static str>,
        connect_ns: Option<u64>,
        first_byte_ns: Option<u64>,
        between_bytes_ns: Option<u64>,
        fail_connect: bool,
        fail_first_byte: bool,
        fail_between_bytes: bool,
    }

    impl RequestDeadlinesSetter for SpyOptions {
        fn set_connect_timeout_ns(&mut self, ns: u64) -> Result<(), ()> {
            self.calls.push("connect");
            if self.fail_connect {
                return Err(());
            }
            self.connect_ns = Some(ns);
            Ok(())
        }

        fn set_first_byte_timeout_ns(&mut self, ns: u64) -> Result<(), ()> {
            self.calls.push("first_byte");
            if self.fail_first_byte {
                return Err(());
            }
            self.first_byte_ns = Some(ns);
            Ok(())
        }

        fn set_between_bytes_timeout_ns(&mut self, ns: u64) -> Result<(), ()> {
            self.calls.push("between_bytes");
            if self.fail_between_bytes {
                return Err(());
            }
            self.between_bytes_ns = Some(ns);
            Ok(())
        }
    }

    #[test]
    fn test_embedded_manifest_pins_exact_imf_host_only() {
        assert_eq!(
            crate::OUTLAYER_MANIFEST.as_slice(),
            EMBEDDED_OUTLAYER_MANIFEST_JSON
        );
        // Parse actual embedded manifest file
        let allowlist = NetworkAllowlist::from_embedded_manifest()
            .expect("embedded outlayer.manifest.json must parse cleanly");
        assert!(!allowlist.is_empty());
        assert_eq!(allowlist.len(), 1);
        assert!(allowlist.allows("api.imf.org"));
        assert!(allowlist.allows("API.IMF.ORG"));
        assert!(!allowlist.allows("example.com"));
        assert!(!allowlist.allows("sub.api.imf.org"));
        assert!(!allowlist.allows("imf.org"));
        assert!(!allowlist.allows("fred.stlouisfed.org"));

        // Inspect raw JSON structure directly
        let val: serde_json::Value =
            serde_json::from_slice(EMBEDDED_OUTLAYER_MANIFEST_JSON).expect("valid JSON");
        assert!(val.get("connector_id").is_none(), "must omit connector_id");
        assert!(
            val.get("author_secrets").is_none(),
            "must omit author_secrets"
        );
        assert!(val.get("operations").is_none(), "must omit operations");
        assert!(val.get("limits").is_none(), "must omit limits");
        let net = val["capabilities"]["network"]
            .as_array()
            .expect("capabilities.network must be an array");
        assert_eq!(net, &[serde_json::json!("api.imf.org")]);

        // Reject missing network, null network, wildcards, connector_id, author_secrets
        for invalid in [
            r#"{}"#,
            r#"{"capabilities":{}}"#,
            r#"{"capabilities":{"network":null}}"#,
            r#"{"capabilities":{"network":["*.example.com"]}}"#,
            r#"{"capabilities":{"network":["127.0.0.1"]}}"#,
            r#"{"connector_id":"roastake","capabilities":{"network":[]}}"#,
            r#"{"author_secrets":{"profile":"prod"},"capabilities":{"network":[]}}"#,
        ] {
            assert!(
                NetworkAllowlist::from_manifest_bytes(invalid.as_bytes()).is_err(),
                "expected InvalidManifest for {invalid}"
            );
        }
    }

    #[test]
    fn test_url_security_and_host_allowlist_rules() {
        let allowlist = NetworkAllowlist::from_hosts(&["api.example.com"]).unwrap();

        // Valid HTTPS exact match (case-insensitive)
        let ok1 = validate_request_url(
            "https://API.Example.COM/v1/coffee?symbol=KC",
            &allowlist,
            Some("test"),
        )
        .unwrap();
        assert_eq!(ok1.normalized_host, "api.example.com");
        assert_eq!(ok1.path_with_query, "/v1/coffee?symbol=KC");

        // HTTP rejected
        assert_eq!(
            validate_request_url("http://api.example.com/v1/coffee", &allowlist, Some("test"))
                .unwrap_err(),
            TransportError::UnsupportedScheme {
                provider: Some("test")
            }
        );

        // Other schemes rejected
        assert_eq!(
            validate_request_url("file:///etc/passwd", &allowlist, Some("test")).unwrap_err(),
            TransportError::UnsupportedScheme {
                provider: Some("test")
            }
        );

        // Embedded credentials rejected
        for cred_url in [
            "https://user:pass@api.example.com/v1",
            "https://apikey@api.example.com/v1",
            "https://api.example.com\\@evil.com/v1",
        ] {
            assert!(
                matches!(
                    validate_request_url(cred_url, &allowlist, Some("test")).unwrap_err(),
                    TransportError::InvalidUrl { .. }
                ),
                "expected InvalidUrl for {cred_url}"
            );
        }

        // Fragments rejected
        assert!(matches!(
            validate_request_url("https://api.example.com/v1#frag", &allowlist, Some("test"))
                .unwrap_err(),
            TransportError::InvalidUrl { .. }
        ));

        // IP literals rejected (IPv4, IPv6, decimal-encoded IP, link-local metadata)
        for ip_url in [
            "https://127.0.0.1/latest",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/latest",
            "https://2130706433/latest",
        ] {
            assert!(
                matches!(
                    validate_request_url(ip_url, &allowlist, Some("test")).unwrap_err(),
                    TransportError::InvalidUrl { .. }
                ),
                "expected IP literal rejection for {ip_url}"
            );
        }

        // Missing hosts and malformed URLs rejected
        for bad_url in [
            "",
            "https:///path",
            "https://:443/path",
            "not a url",
            "https://api..example.com/path",
            "https://*.example.com/path",
        ] {
            assert!(
                matches!(
                    validate_request_url(bad_url, &allowlist, Some("test")).unwrap_err(),
                    TransportError::InvalidUrl { .. }
                ),
                "expected InvalidUrl for {bad_url}"
            );
        }

        // Unapproved host & subdomain non-wildcarding rejected
        for denied in [
            "https://evil.example.com/v1",
            "https://sub.api.example.com/v1",
            "https://example.com/v1",
        ] {
            assert_eq!(
                validate_request_url(denied, &allowlist, Some("test")).unwrap_err(),
                TransportError::HostNotAllowed {
                    provider: Some("test")
                },
                "expected HostNotAllowed for {denied}"
            );
        }

        // Empty allowlist blocks all HTTPS hosts fail-closed
        let empty = NetworkAllowlist::empty();
        assert_eq!(
            validate_request_url(
                "https://api.imf.org/external/sdmx/3.0",
                &empty,
                Some("test")
            )
            .unwrap_err(),
            TransportError::HostNotAllowed {
                provider: Some("test")
            }
        );

        // Embedded allowlist permits only api.imf.org and denies every other host
        let embedded = NetworkAllowlist::from_embedded_manifest().unwrap();
        assert!(validate_request_url(
            "https://api.imf.org/external/sdmx/3.0/data/dataflow/IMF.RES/PCPS/+/G001.PCOFFOTM.USD.M?lastNObservations=1",
            &embedded,
            Some("test"),
        )
        .is_ok());
        for denied in [
            "https://api.example.com/v1",
            "https://fred.stlouisfed.org/graph/fredgraph.csv?id=PCOFFOTMUSDM",
            "https://sub.api.imf.org/v1",
        ] {
            assert_eq!(
                validate_request_url(denied, &embedded, Some("test")).unwrap_err(),
                TransportError::HostNotAllowed {
                    provider: Some("test")
                }
            );
        }
    }

    #[test]
    fn test_timeout_constants_and_wasi_request_options_setter_propagation() {
        assert_eq!(CONNECT_TIMEOUT_SECS, 3);
        assert_eq!(FIRST_BYTE_TIMEOUT_SECS, 8);
        assert_eq!(BETWEEN_BYTES_TIMEOUT_SECS, 8);

        let deadlines = HttpDeadlines::default();
        let mut spy = SpyOptions::default();
        apply_http_deadlines(&mut spy, deadlines, Some("p1")).expect("setters succeed");
        assert_eq!(spy.calls, vec!["connect", "first_byte", "between_bytes"]);
        assert_eq!(spy.connect_ns, Some(3_000_000_000));
        assert_eq!(spy.first_byte_ns, Some(8_000_000_000));
        assert_eq!(spy.between_bytes_ns, Some(8_000_000_000));

        // Setter failure in any of the three options must fail closed
        let mut fail_c = SpyOptions {
            fail_connect: true,
            ..Default::default()
        };
        assert_eq!(
            apply_http_deadlines(&mut fail_c, deadlines, Some("p1")).unwrap_err(),
            TransportError::TimeoutConfigError {
                provider: Some("p1"),
                field: "connect_timeout"
            }
        );

        let mut fail_fb = SpyOptions {
            fail_first_byte: true,
            ..Default::default()
        };
        assert_eq!(
            apply_http_deadlines(&mut fail_fb, deadlines, Some("p1")).unwrap_err(),
            TransportError::TimeoutConfigError {
                provider: Some("p1"),
                field: "first_byte_timeout"
            }
        );

        let mut fail_bb = SpyOptions {
            fail_between_bytes: true,
            ..Default::default()
        };
        assert_eq!(
            apply_http_deadlines(&mut fail_bb, deadlines, Some("p1")).unwrap_err(),
            TransportError::TimeoutConfigError {
                provider: Some("p1"),
                field: "between_bytes_timeout"
            }
        );
    }

    #[test]
    fn test_bounded_response_streaming_and_overflow_checks() {
        assert_eq!(MAX_HTTP_RESPONSE_BYTES, 512 * 1024);

        // Empty response
        let empty = read_chunks_bounded(std::iter::empty::<&[u8]>(), MAX_HTTP_RESPONSE_BYTES, None)
            .unwrap();
        assert!(empty.is_empty());

        // Exact 512 KiB response accepted across multiple chunks
        let half = vec![b'x'; 256 * 1024];
        let exact = read_chunks_bounded(
            [half.as_slice(), half.as_slice()],
            MAX_HTTP_RESPONSE_BYTES,
            Some("p"),
        )
        .unwrap();
        assert_eq!(exact.len(), MAX_HTTP_RESPONSE_BYTES);

        // 512 KiB + 1 byte rejected immediately
        let over_single = vec![b'y'; MAX_HTTP_RESPONSE_BYTES + 1];
        assert_eq!(
            read_chunks_bounded([over_single.as_slice()], MAX_HTTP_RESPONSE_BYTES, Some("p"))
                .unwrap_err(),
            TransportError::ResponseTooLarge {
                provider: Some("p"),
                limit_bytes: MAX_HTTP_RESPONSE_BYTES
            }
        );

        // Chunked body crossing 512 KiB rejected cleanly
        assert_eq!(
            read_chunks_bounded(
                [half.as_slice(), half.as_slice(), b"z"],
                MAX_HTTP_RESPONSE_BYTES,
                Some("p"),
            )
            .unwrap_err(),
            TransportError::ResponseTooLarge {
                provider: Some("p"),
                limit_bytes: MAX_HTTP_RESPONSE_BYTES
            }
        );

        // Checked arithmetic integer overflow guard
        assert_eq!(
            checked_next_len(usize::MAX, 1, usize::MAX, Some("p")).unwrap_err(),
            TransportError::ResponseTooLarge {
                provider: Some("p"),
                limit_bytes: usize::MAX
            }
        );
    }

    #[test]
    fn test_mock_client_timeouts_http_errors_no_retry_and_redaction() {
        let allowlist = NetworkAllowlist::from_hosts(&["api.example.com"]).unwrap();

        // 1. Timeouts (connect, first-byte, between-bytes) propagate with zero retries
        for phase in [
            TimeoutPhase::Connect,
            TimeoutPhase::FirstByte,
            TimeoutPhase::BetweenBytes,
        ] {
            let mock = MockHttpTransport::new();
            mock.enqueue_err(TransportError::Timeout {
                provider: Some("mock-provider"),
                phase,
            });
            let client = HttpClient::new(mock, allowlist.clone());
            let req = HttpRequest::get(
                Some("mock-provider"),
                "https://api.example.com/quote?apikey=TOP_SECRET_123",
            )
            .with_header("Authorization", "Bearer HEADER_SECRET_999");

            let err = client.execute(&req).unwrap_err();
            assert_eq!(
                err,
                TransportError::Timeout {
                    provider: Some("mock-provider"),
                    phase
                }
            );
            assert_eq!(client.transport().recorded_calls().len(), 1);
            assert!(!err.to_string().contains("TOP_SECRET_123"));
            assert!(!format!("{err:?}").contains("HEADER_SECRET_999"));
            assert!(!format!("{req:?}").contains("TOP_SECRET_123"));
            assert!(!format!("{req:?}").contains("HEADER_SECRET_999"));
        }

        // 2. HTTP 401, 429, and 500 propagate without echoing response body
        for status in [401u16, 429u16, 500u16] {
            let mock = MockHttpTransport::new();
            mock.enqueue_ok_json(
                status,
                b"{\"error\":\"invalid token SECRET_BODY_LEAK\"}".to_vec(),
            );
            let client = HttpClient::new(mock, allowlist.clone());
            let req = HttpRequest::get(Some("mock-provider"), "https://api.example.com/quote");
            let err = client.execute(&req).unwrap_err();
            assert_eq!(
                err,
                TransportError::HttpStatus {
                    provider: Some("mock-provider"),
                    status
                }
            );
            assert!(!err.to_string().contains("SECRET_BODY_LEAK"));
            assert!(!format!("{err:?}").contains("SECRET_BODY_LEAK"));
        }

        // 3. Malformed JSON response rejected cleanly with secret-redacted Display/Debug
        let mock_bad_json = MockHttpTransport::new();
        mock_bad_json.enqueue_ok_json(200, b"{not-json SECRET_JSON_TOKEN".to_vec());
        let client_bad_json = HttpClient::new(mock_bad_json, allowlist.clone());
        let ok_resp = client_bad_json
            .execute(&HttpRequest::get(
                Some("mock-provider"),
                "https://api.example.com/quote",
            ))
            .unwrap();
        let json_err = ok_resp
            .json::<serde_json::Value>(Some("mock-provider"))
            .unwrap_err();
        assert_eq!(
            json_err,
            TransportError::MalformedResponse {
                provider: Some("mock-provider"),
                reason: "failed to parse JSON response body"
            }
        );
        assert!(!json_err.to_string().contains("SECRET_JSON_TOKEN"));
        assert!(!format!("{ok_resp:?}").contains("SECRET_JSON_TOKEN"));

        // 4. POST request with body + bounded response success
        let mock_post = MockHttpTransport::new();
        mock_post.enqueue_ok_chunks(200, vec![br#"{"price":"#.to_vec(), br#""7.27"}"#.to_vec()]);
        let client_post = HttpClient::new(mock_post, allowlist);
        let post_req = HttpRequest::post(
            Some("mock-provider"),
            "https://api.example.com/rpc",
            Some(br#"{"query":"coffee"}"#.to_vec()),
        )
        .with_header("Content-Type", "application/json");
        let resp = client_post.execute(&post_req).unwrap();
        assert_eq!(resp.status, 200);
        let parsed: serde_json::Value = resp.json(Some("mock-provider")).unwrap();
        assert_eq!(parsed["price"], "7.27");
    }
}
