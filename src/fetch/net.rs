//! Shared HTTP client construction, Hugging Face Hub endpoint/token
//! resolution, and resumable downloads for `parakit fetch`.
//!
//! Corporate-network support is a hard requirement here: the crate already
//! builds `reqwest` with `rustls-tls-native-roots` and `system-proxy`, so the
//! OS trust store (including TLS-interception CAs some networks install) and
//! system proxy settings are honored without any code in this module. What
//! this module adds on top is `HF_ENDPOINT` (an internal mirror override),
//! `HF_TOKEN` (bearer auth for gated repos or authenticated mirrors), and a
//! certificate-failure hint appended to the error chain so a blocked
//! `huggingface.co` or a TLS-intercepting proxy produces actionable guidance
//! instead of an opaque `rustls`/`reqwest` error.

use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{
    HeaderMap, HeaderValue, AUTHORIZATION, ETAG, IF_RANGE, LAST_MODIFIED, RANGE, USER_AGENT,
};
use reqwest::StatusCode;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::model::{HF_DEFAULT_ENDPOINT, HF_ENDPOINT_ENV, HF_TOKEN_ENV};

/// Resolve the Hugging Face Hub endpoint once per `fetch` invocation.
///
/// Read exactly once at the top of [`super::run`] and threaded down as a
/// parameter from there, rather than re-read inside helpers: that keeps every
/// URL-building and auth-decision function a pure, environment-free unit
/// under test.
///
/// # Returns
///
/// `HF_ENDPOINT` with its trailing slash trimmed when set to a non-blank
/// value, otherwise [`HF_DEFAULT_ENDPOINT`].
pub(super) fn resolve_endpoint() -> String {
    match std::env::var(HF_ENDPOINT_ENV) {
        Ok(value) if !value.trim().is_empty() => normalize_endpoint(&value),
        _ => HF_DEFAULT_ENDPOINT.to_string(),
    }
}

/// Trim a trailing `/` (or run of them) from a configured endpoint so URL
/// building never produces a doubled slash.
fn normalize_endpoint(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

/// Resolve the optional Hugging Face auth token once per `fetch` invocation.
/// See [`resolve_endpoint`] for why this is read once at entry.
///
/// # Returns
///
/// `Some(token)` when `HF_TOKEN` is set to a non-blank value, else `None`.
pub(super) fn resolve_token() -> Option<String> {
    std::env::var(HF_TOKEN_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Rewrite a pinned `https://huggingface.co/...` URL onto a non-default
/// resolved endpoint.
///
/// # Arguments
///
/// * `url` - A pinned URL (`HOSTED_Q8_URL` or `OFFICIAL_NEMO_URL`).
/// * `endpoint` - The resolved Hub endpoint (see [`resolve_endpoint`]).
///
/// # Returns
///
/// `url` with a leading [`HF_DEFAULT_ENDPOINT`] replaced by `endpoint`, or
/// `url` unchanged when `endpoint` is still the public Hub or `url` does not
/// start with the expected prefix.
pub(super) fn rewrite_pinned_url(url: &str, endpoint: &str) -> String {
    if endpoint == HF_DEFAULT_ENDPOINT {
        return url.to_string();
    }
    match url.strip_prefix(HF_DEFAULT_ENDPOINT) {
        Some(rest) => format!("{endpoint}{rest}"),
        None => url.to_string(),
    }
}

/// Decide whether a bearer token should be attached to a request for `url`.
///
/// # Arguments
///
/// * `url` - The request URL under consideration.
/// * `endpoint` - The resolved Hub endpoint (see [`resolve_endpoint`]).
/// * `token` - The resolved Hub token (see [`resolve_token`]), if any.
///
/// # Returns
///
/// `token` when `url` targets the resolved Hub `endpoint`, else `None`. The
/// match requires a path boundary after the endpoint prefix (`/`, `?`, or
/// end of string), so `https://huggingface.co.evil.example` never matches an
/// endpoint of `https://huggingface.co`.
///
/// # Note
///
/// Callers handling an arbitrary user-supplied `--url` fetch source must
/// never call this: pass `bearer: None` directly to [`download_with_resume`]
/// so a coincidental host match can never leak the token to a host the user
/// only asked to download a file from.
pub(super) fn bearer_for<'a>(url: &str, endpoint: &str, token: Option<&'a str>) -> Option<&'a str> {
    token.filter(|_| {
        url.strip_prefix(endpoint)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('?'))
    })
}

/// Attach a bearer `Authorization` header to `request` when `bearer` is set.
///
/// # Arguments
///
/// * `request` - The in-progress request builder.
/// * `bearer` - The token to attach, if any (see [`bearer_for`]).
///
/// # Returns
///
/// `request` with an `Authorization: Bearer <token>` header added when
/// `bearer` is `Some`, otherwise `request` unchanged.
pub(super) fn attach_bearer(request: RequestBuilder, bearer: Option<&str>) -> RequestBuilder {
    match bearer {
        Some(token) => request.header(AUTHORIZATION, format!("Bearer {token}")),
        None => request,
    }
}

fn default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(concat!("parakit/", env!("CARGO_PKG_VERSION"))),
    );
    headers
}

/// Build the shared blocking HTTP client used for both the Hub API call and
/// downloads.
///
/// # Returns
///
/// A client with parakit's User-Agent header set, ready to issue requests.
///
/// # Errors
///
/// Returns an error if the underlying TLS/HTTP client cannot be constructed.
pub(super) fn build_client() -> Result<Client> {
    Client::builder()
        .default_headers(default_headers())
        .build()
        .context("build HTTP client")
}

const CERT_HINT: &str = "parakit trusts the operating-system certificate store (rustls-tls-native-roots). Behind a TLS-intercepting proxy: install the proxy's CA into the OS trust store, or point SSL_CERT_FILE (or SSL_CERT_DIR) at the corporate PEM bundle. If an egress proxy is required, set HTTPS_PROXY/HTTP_PROXY (system proxy settings are already honored). If huggingface.co is blocked outright, set HF_ENDPOINT to an internal mirror.";

/// Append [`CERT_HINT`] to the error chain when `result` failed with what
/// looks like a TLS/certificate failure, otherwise return it unchanged.
///
/// Applied at the boundary of both HTTP-calling primitives in this module
/// ([`download_with_resume`] and the Hub API GET in
/// `super::hub::fetch_repo_metadata`) so the guidance is available for every
/// fetch path, including the pre-existing hosted-Q8 and official-`.nemo`
/// downloads, without duplicating the matching logic at each call site.
///
/// # Returns
///
/// `result` with [`CERT_HINT`] appended via [`anyhow::Context`] when the
/// error chain matched, otherwise `result` unchanged. The hint lives on the
/// error chain, not stdout, so it survives `?` propagation to the top-level
/// error report.
///
/// # Errors
///
/// Returns `result`'s original error (with [`CERT_HINT`] appended when it
/// looked TLS/certificate-related); never fails on its own.
pub(super) fn with_cert_hint<T>(result: Result<T>) -> Result<T> {
    result.map_err(|err| {
        if chain_looks_like_cert_failure(&err) {
            err.context(CERT_HINT)
        } else {
            err
        }
    })
}

fn chain_looks_like_cert_failure(err: &anyhow::Error) -> bool {
    const NEEDLES: [&str; 4] = ["certificate", "unknownissuer", "invalid peer", "handshake"];
    err.chain().any(|cause| {
        let text = cause.to_string().to_ascii_lowercase();
        NEEDLES.iter().any(|needle| text.contains(needle))
    })
}

/// Download `url` to `path`, resuming an existing partial file with an HTTP
/// `Range` request when possible and restarting from scratch when the server
/// rejects the range.
///
/// # Arguments
///
/// * `client` - HTTP client from [`build_client`].
/// * `url` - URL to download.
/// * `path` - Destination file; appended to if it already exists.
/// * `bearer` - Bearer token to attach, if any (see [`bearer_for`]).
///
/// # Returns
///
/// `Ok(())` once `path` holds the fully downloaded content.
///
/// # Errors
///
/// Returns an error (with [`CERT_HINT`] appended when the failure looks
/// TLS/certificate-related) if the request fails, the server returns an
/// unexpected status, or `path` cannot be written.
pub(super) fn download_with_resume(
    client: &Client,
    url: &str,
    path: &Path,
    bearer: Option<&str>,
) -> Result<()> {
    with_cert_hint(download_with_resume_inner(client, url, path, bearer))
}

fn download_with_resume_inner(
    client: &Client,
    url: &str,
    path: &Path,
    bearer: Option<&str>,
) -> Result<()> {
    let (mut start, request) = prepare_download_request(client, url, path, bearer)?;

    let mut response = request.send().with_context(|| format!("GET {url}"))?;
    match response.status() {
        StatusCode::OK => {
            if start > 0 {
                start = 0;
            }
        }
        StatusCode::PARTIAL_CONTENT if start > 0 => {}
        StatusCode::PARTIAL_CONTENT => {
            bail!("download returned partial content without a range request")
        }
        StatusCode::RANGE_NOT_SATISFIABLE => {
            super::remove_if_exists(path)?;
            let _ = super::remove_if_exists(&resume_validator_path(path));
            response = attach_bearer(client.get(url), bearer)
                .send()
                .with_context(|| format!("GET {url}"))?;
            if response.status() != StatusCode::OK {
                bail!(
                    "download restart failed with HTTP status {}",
                    response.status()
                );
            }
            start = 0;
        }
        status => {
            bail!("download failed with HTTP status {status}");
        }
    }

    let validator_path = resume_validator_path(path);
    if start == 0 {
        save_resume_validator(&validator_path, response.headers());
    }

    let mut file = if start == 0 {
        File::create(path).with_context(|| format!("create {}", path.display()))?
    } else {
        OpenOptions::new()
            .append(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?
    };
    std::io::copy(&mut response, &mut file)?;
    file.flush()?;
    let _ = super::remove_if_exists(&validator_path);
    Ok(())
}

fn prepare_download_request(
    client: &Client,
    url: &str,
    path: &Path,
    bearer: Option<&str>,
) -> Result<(u64, RequestBuilder)> {
    let validator_path = resume_validator_path(path);
    let mut start = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let validator = if start > 0 {
        load_resume_validator(&validator_path)
    } else {
        let _ = super::remove_if_exists(&validator_path);
        None
    };

    if start > 0 && validator.is_none() {
        super::remove_if_exists(path)?;
        start = 0;
    }

    let mut request = attach_bearer(client.get(url), bearer);
    if let Some(validator) = validator {
        request = request
            .header(RANGE, format!("bytes={start}-"))
            .header(IF_RANGE, validator);
    }
    Ok((start, request))
}

fn resume_validator_path(path: &Path) -> PathBuf {
    let mut validator = path.as_os_str().to_os_string();
    validator.push(".validator");
    PathBuf::from(validator)
}

fn load_resume_validator(path: &Path) -> Option<HeaderValue> {
    let bytes = std::fs::read(path).ok()?;
    HeaderValue::from_bytes(&bytes).ok()
}

fn save_resume_validator(path: &Path, headers: &HeaderMap) {
    let strong_etag = headers
        .get(ETAG)
        .filter(|value| !value.as_bytes().starts_with(b"W/"));
    let validator = strong_etag.or_else(|| headers.get(LAST_MODIFIED));
    let _ = super::remove_if_exists(path);
    if let Some(value) = validator {
        let _ = std::fs::write(path, value.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_endpoint_trims_trailing_slash() {
        assert_eq!(
            normalize_endpoint("https://mirror.internal.example.com/"),
            "https://mirror.internal.example.com"
        );
        assert_eq!(
            normalize_endpoint("https://mirror.internal.example.com"),
            "https://mirror.internal.example.com"
        );
    }

    #[test]
    fn rewrite_pinned_url_cases() {
        let pinned = "https://huggingface.co/pszemraj/model/resolve/main/model.gguf";
        for (name, url, endpoint, expect) in [
            (
                "swaps the huggingface prefix",
                pinned,
                "https://mirror.internal.example.com",
                "https://mirror.internal.example.com/pszemraj/model/resolve/main/model.gguf",
            ),
            (
                "is identity on the default endpoint",
                pinned,
                HF_DEFAULT_ENDPOINT,
                pinned,
            ),
            (
                "leaves a non-matching URL alone",
                "https://example.com/model.gguf",
                "https://mirror.internal.example.com",
                "https://example.com/model.gguf",
            ),
        ] {
            assert_eq!(rewrite_pinned_url(url, endpoint), expect, "{name}");
        }
    }

    #[test]
    fn bearer_for_cases() {
        for (name, url, endpoint, token, expect) in [
            (
                "matches a request under the resolved endpoint",
                "https://huggingface.co/api/models/a/b/revision/main",
                "https://huggingface.co",
                Some("tok"),
                Some("tok"),
            ),
            (
                "does not match an unrelated host",
                "https://example.com/model.gguf",
                "https://huggingface.co",
                Some("tok"),
                None,
            ),
            (
                "no token means nothing to attach even on a matching host",
                "https://huggingface.co/a/b",
                "https://huggingface.co",
                None,
                None,
            ),
            (
                "requires a path boundary after the endpoint prefix",
                "https://huggingface.co.evil.example/a/b",
                "https://huggingface.co",
                Some("tok"),
                None,
            ),
            (
                "matches the endpoint URL itself with no path",
                "https://huggingface.co",
                "https://huggingface.co",
                Some("tok"),
                Some("tok"),
            ),
        ] {
            assert_eq!(bearer_for(url, endpoint, token), expect, "{name}");
        }
    }

    #[test]
    fn resume_request_pairs_range_with_the_saved_validator() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "resume-validator");
        let partial = dir.join("model.gguf.part");
        std::fs::write(&partial, b"partial").unwrap();
        let mut response_headers = HeaderMap::new();
        response_headers.insert(ETAG, HeaderValue::from_static("\"revision-1\""));
        save_resume_validator(&resume_validator_path(&partial), &response_headers);

        let client = build_client().unwrap();
        let (start, request) =
            prepare_download_request(&client, "https://example.com/model.gguf", &partial, None)
                .unwrap();
        let request = request.build().unwrap();

        assert_eq!(start, 7);
        assert_eq!(request.headers().get(RANGE).unwrap(), "bytes=7-");
        assert_eq!(request.headers().get(IF_RANGE).unwrap(), "\"revision-1\"");
    }

    #[test]
    fn partial_without_a_validator_restarts_from_scratch() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "resume-no-validator");
        let partial = dir.join("model.gguf.part");
        std::fs::write(&partial, b"stale").unwrap();

        let client = build_client().unwrap();
        let (start, request) =
            prepare_download_request(&client, "https://example.com/model.gguf", &partial, None)
                .unwrap();
        let request = request.build().unwrap();

        assert_eq!(start, 0);
        assert!(!partial.exists());
        assert!(!request.headers().contains_key(RANGE));
        assert!(!request.headers().contains_key(IF_RANGE));
    }

    #[test]
    fn partial_with_an_invalid_validator_restarts_from_scratch() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "resume-bad-validator");
        let partial = dir.join("model.gguf.part");
        std::fs::write(&partial, b"stale").unwrap();
        std::fs::write(resume_validator_path(&partial), b"bad\nheader").unwrap();

        let client = build_client().unwrap();
        let (start, request) =
            prepare_download_request(&client, "https://example.com/model.gguf", &partial, None)
                .unwrap();
        let request = request.build().unwrap();

        assert_eq!(start, 0);
        assert!(!partial.exists());
        assert!(!request.headers().contains_key(RANGE));
        assert!(!request.headers().contains_key(IF_RANGE));
    }

    #[test]
    fn with_cert_hint_appends_guidance_for_tls_failures_and_leaves_others_alone() {
        let tls_phrasings = [
            "error sending request: unable to get local issuer certificate",
            "invalid peer certificate: UnknownIssuer",
            "tls handshake eof",
            "SEC_E_UNTRUSTED_ROOT: the certificate chain was issued by an authority that is not trusted",
        ];
        for phrasing in tls_phrasings {
            let matching: Result<()> = Err(anyhow::anyhow!("{phrasing}"));
            let wrapped = with_cert_hint(matching).unwrap_err();
            assert!(
                format!("{wrapped:#}").contains("operating-system certificate store"),
                "expected a cert hint for: {phrasing}"
            );
        }

        let unrelated: Result<()> = Err(anyhow::anyhow!("connection refused"));
        let wrapped = with_cert_hint(unrelated).unwrap_err();
        assert!(!format!("{wrapped:#}").contains("operating-system certificate store"));
    }
}
