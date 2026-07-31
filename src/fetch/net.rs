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
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, RANGE, USER_AGENT};
use reqwest::StatusCode;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

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
    let mut start = path.metadata().map(|m| m.len()).unwrap_or(0);
    let mut request = attach_bearer(client.get(url), bearer);
    if start > 0 {
        request = request.header(RANGE, format!("bytes={start}-"));
    }

    let mut response = request.send().with_context(|| format!("GET {url}"))?;
    match response.status() {
        StatusCode::OK => {
            if start > 0 {
                start = 0;
            }
        }
        StatusCode::PARTIAL_CONTENT => {}
        StatusCode::RANGE_NOT_SATISFIABLE => {
            super::remove_if_exists(path)?;
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
    Ok(())
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
    fn rewrite_pinned_url_swaps_the_huggingface_prefix() {
        assert_eq!(
            rewrite_pinned_url(
                "https://huggingface.co/pszemraj/model/resolve/main/model.gguf",
                "https://mirror.internal.example.com"
            ),
            "https://mirror.internal.example.com/pszemraj/model/resolve/main/model.gguf"
        );
    }

    #[test]
    fn rewrite_pinned_url_is_identity_on_the_default_endpoint() {
        let url = "https://huggingface.co/pszemraj/model/resolve/main/model.gguf";
        assert_eq!(rewrite_pinned_url(url, HF_DEFAULT_ENDPOINT), url);
    }

    #[test]
    fn rewrite_pinned_url_leaves_a_non_matching_url_alone() {
        let url = "https://example.com/model.gguf";
        assert_eq!(
            rewrite_pinned_url(url, "https://mirror.internal.example.com"),
            url
        );
    }

    #[test]
    fn bearer_for_matches_only_the_resolved_endpoint_host() {
        let endpoint = "https://huggingface.co";
        assert_eq!(
            bearer_for(
                "https://huggingface.co/api/models/a/b/revision/main",
                endpoint,
                Some("tok")
            ),
            Some("tok")
        );
        assert_eq!(
            bearer_for("https://example.com/model.gguf", endpoint, Some("tok")),
            None
        );
        assert_eq!(
            bearer_for("https://huggingface.co/a/b", endpoint, None),
            None
        );
    }

    #[test]
    fn bearer_for_requires_a_path_boundary_after_the_endpoint_prefix() {
        let endpoint = "https://huggingface.co";
        assert_eq!(
            bearer_for(
                "https://huggingface.co.evil.example/a/b",
                endpoint,
                Some("tok")
            ),
            None
        );
        assert_eq!(bearer_for(endpoint, endpoint, Some("tok")), Some("tok"));
    }

    #[test]
    fn cert_hint_matches_common_tls_failure_phrasings() {
        let cases = [
            "error sending request: unable to get local issuer certificate",
            "invalid peer certificate: UnknownIssuer",
            "tls handshake eof",
            "SEC_E_UNTRUSTED_ROOT: the certificate chain was issued by an authority that is not trusted",
        ];
        for case in cases {
            let err = anyhow::anyhow!("{case}");
            assert!(
                chain_looks_like_cert_failure(&err),
                "expected a cert-hint match for: {case}"
            );
        }
    }

    #[test]
    fn cert_hint_does_not_match_unrelated_errors() {
        let err = anyhow::anyhow!("connection refused");
        assert!(!chain_looks_like_cert_failure(&err));
    }

    #[test]
    fn with_cert_hint_appends_guidance_only_on_a_matching_chain() {
        let matching: Result<()> = Err(anyhow::anyhow!("invalid peer certificate: UnknownIssuer"));
        let wrapped = with_cert_hint(matching).unwrap_err();
        assert!(format!("{wrapped:#}").contains("operating-system certificate store"));

        let unrelated: Result<()> = Err(anyhow::anyhow!("connection refused"));
        let wrapped = with_cert_hint(unrelated).unwrap_err();
        assert!(!format!("{wrapped:#}").contains("operating-system certificate store"));
    }
}
