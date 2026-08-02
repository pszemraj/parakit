//! Hugging Face repo source parsing, Hub API lookup, `.gguf` file selection,
//! and the repo acquisition flow for `parakit fetch <owner>/<repo>`.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::{net, FetchOptions, FetchSource};

/// A parsed Hugging Face repo specification: `owner/repo` or
/// `owner/repo@revision`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoSpec {
    owner: String,
    repo: String,
    /// `None` means "use the Hub default (`main`)"; kept distinct from
    /// `Some("main")` so the two spellings round-trip through
    /// [`FetchSource::HubRepo`] without inventing a revision the user did
    /// not ask for.
    revision: Option<String>,
}

/// How a `parakit fetch` positional argument was classified.
#[derive(Debug)]
enum SourceKind {
    Repo(RepoSpec),
    Url(String),
}

/// Classify a `parakit fetch` positional argument as an HTTP(S) URL or a
/// Hugging Face repo spec.
fn classify_source(input: &str) -> Result<SourceKind> {
    if let Some((scheme, _)) = input.split_once("://") {
        return if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            Ok(SourceKind::Url(input.to_string()))
        } else {
            Err(invalid_source_error(input))
        };
    }
    parse_repo_spec(input).map(SourceKind::Repo)
}

fn parse_repo_spec(input: &str) -> Result<RepoSpec> {
    let (repo_part, revision) = match input.rsplit_once('@') {
        Some((_, "")) => return Err(invalid_source_error(input)),
        Some((head, rev)) => (head, Some(rev.to_string())),
        None => (input, None),
    };

    let mut parts = repo_part.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid_source_error(input));
    };
    if owner.is_empty()
        || repo.is_empty()
        || !is_valid_repo_component(owner)
        || !is_valid_repo_component(repo)
    {
        return Err(invalid_source_error(input));
    }

    Ok(RepoSpec {
        owner: owner.to_string(),
        repo: repo.to_string(),
        revision,
    })
}

fn is_valid_repo_component(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn invalid_source_error(input: &str) -> anyhow::Error {
    anyhow!(
        "invalid fetch source '{input}': expected a Hugging Face repo (owner/repo or owner/repo@revision) or a http(s):// URL"
    )
}

/// Build a [`FetchSource`] from `parakit fetch`'s raw CLI fields.
///
/// # Arguments
///
/// * `source` - The positional `REPO_OR_URL` argument, if given.
/// * `file` - `--file`, only meaningful for a Hugging Face repo `source`.
/// * `sha256` - `--sha256`, valid for either kind of `source`.
///
/// # Returns
///
/// [`FetchSource::HostedQ8`] when `source` is absent, otherwise
/// [`FetchSource::HubRepo`] or [`FetchSource::Url`] depending on how
/// `source` classifies.
///
/// # Errors
///
/// Returns an error when `source` is neither a valid repo spec nor a URL, or
/// when `--file` is paired with a URL `source` (a repo-only selector).
pub fn source_from_cli(
    source: Option<String>,
    file: Option<String>,
    sha256: Option<String>,
) -> Result<FetchSource> {
    let Some(source) = source else {
        return Ok(FetchSource::HostedQ8);
    };

    match classify_source(&source)? {
        SourceKind::Url(url) => {
            if file.is_some() {
                bail!(
                    "--file requires a Hugging Face repo source (owner/repo), not a URL: {source}"
                );
            }
            Ok(FetchSource::Url { url, sha256 })
        }
        SourceKind::Repo(spec) => Ok(FetchSource::HubRepo {
            repo: format!("{}/{}", spec.owner, spec.repo),
            revision: spec.revision,
            file,
            sha256,
        }),
    }
}

/// One `siblings[]` entry from the Hub `GET /api/models/{repo}` response.
#[derive(Debug, Deserialize, Clone)]
struct Sibling {
    rfilename: String,
}

#[derive(Debug, Deserialize)]
struct ApiModelResponse {
    #[serde(default)]
    siblings: Vec<Sibling>,
}

fn is_gguf_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".gguf")
}

/// The chosen `.gguf` sibling and whether it was picked, distinguished only
/// for the case a caller needs to know about: auto-selection as the sole
/// Q8_0 match, which gets a status line no other pick does.
#[derive(Debug)]
enum Selection<'a> {
    /// Picked via an exact `--file` match, or as the sole `.gguf` file.
    Picked(&'a Sibling),
    /// Auto-picked as the sole Q8_0 match among several `.gguf` files.
    AutoQ8(&'a Sibling),
}

impl<'a> Selection<'a> {
    fn sibling(&self) -> &'a Sibling {
        match self {
            Selection::Picked(s) | Selection::AutoQ8(s) => s,
        }
    }
}

/// Select which `.gguf` sibling to download.
///
/// # Returns
///
/// `--file`'s exact match when `requested` is set; the sole `.gguf` file
/// when there is exactly one; the sole file whose name contains `q8_0`
/// (case-insensitive) when the rest is ambiguous; otherwise an error listing
/// every `.gguf` file found and asking for `--file`.
fn select_gguf_file<'a>(
    gguf_files: &[&'a Sibling],
    requested: Option<&str>,
) -> Result<Selection<'a>> {
    if let Some(name) = requested {
        return gguf_files
            .iter()
            .find(|s| s.rfilename == name)
            .map(|s| Selection::Picked(s))
            .ok_or_else(|| {
                anyhow!(
                    "file '{name}' not found in repo; available .gguf files: {}",
                    list_names(gguf_files)
                )
            });
    }

    if gguf_files.is_empty() {
        bail!("no .gguf files found in repo");
    }
    if gguf_files.len() == 1 {
        return Ok(Selection::Picked(gguf_files[0]));
    }

    let q8_matches: Vec<&&Sibling> = gguf_files
        .iter()
        .filter(|s| s.rfilename.to_ascii_lowercase().contains("q8_0"))
        .collect();
    if q8_matches.len() == 1 {
        return Ok(Selection::AutoQ8(q8_matches[0]));
    }

    bail!(
        "multiple .gguf files found; pass --file to choose one: {}",
        list_names(gguf_files)
    );
}

fn list_names(siblings: &[&Sibling]) -> String {
    siblings
        .iter()
        .map(|s| s.rfilename.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn percent_encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Percent-encode a `/`-separated path, encoding each segment independently
/// so the separators themselves stay literal.
fn percent_encode_path(path: &str) -> String {
    path.split('/')
        .map(percent_encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn api_url(endpoint: &str, owner: &str, repo: &str, revision: &str) -> String {
    format!(
        "{endpoint}/api/models/{}/{}/revision/{}?blobs=true",
        percent_encode_segment(owner),
        percent_encode_segment(repo),
        percent_encode_path(revision),
    )
}

fn resolve_url(endpoint: &str, owner: &str, repo: &str, revision: &str, rfilename: &str) -> String {
    format!(
        "{endpoint}/{}/{}/resolve/{}/{}",
        percent_encode_segment(owner),
        percent_encode_segment(repo),
        percent_encode_path(revision),
        percent_encode_path(rfilename),
    )
}

fn hub_cache_dir(models_dir: &Path, owner: &str, repo: &str, revision: &str) -> PathBuf {
    let revision_key = crate::checksum::hex_digest(&Sha256::digest(revision.as_bytes()));
    models_dir
        .join("hub")
        .join(format!("{owner}--{repo}--{revision_key}"))
}

fn hub_file_name(rfilename: &str) -> Result<&str> {
    let basename = rfilename.rsplit(['/', '\\']).next().unwrap_or("");
    super::validate_download_file_name(basename)
        .with_context(|| format!("repo file '{rfilename}' has no usable file name"))?;
    Ok(basename)
}

fn fetch_repo_metadata(
    client: &Client,
    endpoint: &str,
    token: Option<&str>,
    owner: &str,
    name: &str,
    revision: &str,
) -> Result<ApiModelResponse> {
    let url = api_url(endpoint, owner, name, revision);
    let bearer = net::bearer_for(&url, endpoint, token);
    let result = (|| -> Result<ApiModelResponse> {
        let request = net::attach_bearer(client.get(&url), bearer);
        let response = request.send().with_context(|| format!("GET {url}"))?;
        if response.status() == StatusCode::NOT_FOUND {
            bail!("repo or revision not found: {owner}/{name}@{revision}");
        }
        if !response.status().is_success() {
            bail!("GET {url} failed with HTTP status {}", response.status());
        }
        let text = response
            .text()
            .with_context(|| format!("read response body from {url}"))?;
        serde_json::from_str(&text).with_context(|| format!("parse Hub API response from {url}"))
    })();
    net::with_cert_hint(result)
}

/// Run the Hugging Face repo acquisition flow: look up `repo`'s `.gguf`
/// siblings, pick one, and download it into
/// a revision-specific directory under `models_dir()/hub/`.
///
/// # Arguments
///
/// * `options` - Shared fetch options (`force`, status verbosity).
/// * `endpoint` - Resolved Hub endpoint (see [`net::resolve_endpoint`]).
/// * `token` - Resolved Hub bearer token, if any.
/// * `repo` - `owner/repo`.
/// * `revision` - Revision to fetch from; `None` defers to `main`.
/// * `file` - Exact `rfilename` to select; `None` triggers automatic selection.
/// * `sha256_override` - User-supplied `--sha256`, when present.
///
/// # Returns
///
/// The path the selected `.gguf` file was downloaded (or was already cached) to.
///
/// # Errors
///
/// Returns an error if `repo` is malformed, the Hub lookup fails (including
/// a 404 for an unknown repo or revision), no `.gguf` file can be
/// unambiguously selected, or the download/verification fails.
pub(super) fn run_hub_repo(
    options: &FetchOptions,
    endpoint: &str,
    token: Option<&str>,
    repo: &str,
    revision: Option<&str>,
    file: Option<&str>,
    sha256_override: Option<&str>,
) -> Result<PathBuf> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow!("invalid repo '{repo}': expected owner/repo"))?;
    let revision = revision.unwrap_or("main");

    let client = net::build_client()?;
    let metadata = fetch_repo_metadata(&client, endpoint, token, owner, name, revision)?;
    let gguf_files: Vec<&Sibling> = metadata
        .siblings
        .iter()
        .filter(|s| is_gguf_name(&s.rfilename))
        .collect();
    let selection = select_gguf_file(&gguf_files, file)?;
    if let Selection::AutoQ8(sibling) = &selection {
        options.status(format_args!(
            "parakit: auto-selected {} (the only Q8_0 match; Q8_0 is parakit's default quant)",
            sibling.rfilename
        ));
    }
    let sibling = selection.sibling();

    let models_dir = crate::model::models_dir()?;
    let dest_dir = hub_cache_dir(&models_dir, owner, name, revision);
    std::fs::create_dir_all(&dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    let basename = hub_file_name(&sibling.rfilename)?;
    let dest = dest_dir.join(basename);

    let resolve = resolve_url(endpoint, owner, name, revision, &sibling.rfilename);
    if !options.force && super::use_cached_download(options, &dest, sha256_override)? {
        return Ok(dest);
    }

    let bearer = net::bearer_for(&resolve, endpoint, token);
    options.status(format_args!("parakit: downloading {resolve}"));
    super::download_and_verify(options, &client, &resolve, &dest, bearer, sha256_override)?;

    super::print_ready_with_hint(options, &dest);
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_repo() {
        let SourceKind::Repo(spec) = classify_source("cstr/parakeet-tdt-0.6b-v3-GGUF").unwrap()
        else {
            panic!("expected a repo spec");
        };
        assert_eq!(spec.owner, "cstr");
        assert_eq!(spec.repo, "parakeet-tdt-0.6b-v3-GGUF");
        assert_eq!(spec.revision, None);
    }

    #[test]
    fn parses_owner_repo_with_revision() {
        let SourceKind::Repo(spec) =
            classify_source("handy-computer/parakeet-tdt-0.6b-v3-gguf@refs-pr-1").unwrap()
        else {
            panic!("expected a repo spec");
        };
        assert_eq!(spec.owner, "handy-computer");
        assert_eq!(spec.repo, "parakeet-tdt-0.6b-v3-gguf");
        assert_eq!(spec.revision.as_deref(), Some("refs-pr-1"));
    }

    #[test]
    fn detects_a_url_source_before_attempting_repo_parsing() {
        let SourceKind::Url(url) =
            classify_source("https://example.com/models/parakeet-q8.gguf").unwrap()
        else {
            panic!("expected a URL source");
        };
        assert_eq!(url, "https://example.com/models/parakeet-q8.gguf");
    }

    #[test]
    fn rejects_non_http_url_schemes_before_network_io() {
        for source in [
            "file:///tmp/model.gguf",
            "ftp://example.com/model.gguf",
            "custom://example.com/model.gguf",
        ] {
            let error = classify_source(source).expect_err("source should fail");
            assert!(error.to_string().contains("http(s):// URL"), "{source}");
        }
    }

    #[test]
    fn rejects_garbage_specs() {
        for bad in [
            "",
            "owner",
            "owner/",
            "/repo",
            "a/b/c",
            "owner/repo@",
            "ow ner/repo",
            "ow!ner/repo",
            "owner/re/po",
        ] {
            assert!(
                classify_source(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn source_from_cli_defaults_to_hosted_q8() {
        let source = source_from_cli(None, None, None).unwrap();
        assert_eq!(source, FetchSource::HostedQ8);
    }

    #[test]
    fn source_from_cli_builds_a_hub_repo_source() {
        let source = source_from_cli(
            Some("cstr/parakeet-tdt-0.6b-v3-GGUF".to_string()),
            Some("parakeet-tdt-0.6b-v3-q4_k.gguf".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(
            source,
            FetchSource::HubRepo {
                repo: "cstr/parakeet-tdt-0.6b-v3-GGUF".to_string(),
                revision: None,
                file: Some("parakeet-tdt-0.6b-v3-q4_k.gguf".to_string()),
                sha256: None,
            }
        );
    }

    #[test]
    fn source_from_cli_builds_a_url_source() {
        let sha = "a".repeat(64);
        let source = source_from_cli(
            Some("https://example.com/model.gguf".to_string()),
            None,
            Some(sha.clone()),
        )
        .unwrap();
        assert_eq!(
            source,
            FetchSource::Url {
                url: "https://example.com/model.gguf".to_string(),
                sha256: Some(sha),
            }
        );
    }

    #[test]
    fn source_from_cli_rejects_file_with_a_url_source() {
        let err = source_from_cli(
            Some("https://example.com/model.gguf".to_string()),
            Some("model.gguf".to_string()),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("--file"));
        assert!(err.to_string().contains("URL"));
    }

    fn sibling(rfilename: &str) -> Sibling {
        Sibling {
            rfilename: rfilename.to_string(),
        }
    }

    /// Expected outcome of [`select_gguf_file`] for one [`SelectCase`] row.
    /// `Err` carries every message fragment the original per-scenario test
    /// asserted with `.contains()`.
    #[derive(Debug)]
    enum ExpectedSelection {
        Picked(&'static str),
        AutoQ8(&'static str),
        Err(&'static [&'static str]),
    }

    struct SelectCase {
        name: &'static str,
        files: &'static [&'static str],
        requested: Option<&'static str>,
        expect: ExpectedSelection,
    }

    #[test]
    fn select_gguf_file_cases() {
        let cases = [
            SelectCase {
                name: "selects the sole .gguf file",
                files: &["model-Q8_0.gguf"],
                requested: None,
                expect: ExpectedSelection::Picked("model-Q8_0.gguf"),
            },
            SelectCase {
                name: "explicit --file hits an exact match",
                files: &["model-Q8_0.gguf", "model-Q4_K_M.gguf"],
                requested: Some("model-Q4_K_M.gguf"),
                expect: ExpectedSelection::Picked("model-Q4_K_M.gguf"),
            },
            SelectCase {
                name: "explicit --file miss lists available files",
                files: &["model-Q8_0.gguf"],
                requested: Some("nope.gguf"),
                expect: ExpectedSelection::Err(&["model-Q8_0.gguf"]),
            },
            SelectCase {
                name: "auto-picks the sole Q8_0 file among several",
                files: &["model-Q8_0.gguf", "model-Q4_K_M.gguf", "model-F16.gguf"],
                requested: None,
                expect: ExpectedSelection::AutoQ8("model-Q8_0.gguf"),
            },
            SelectCase {
                name: "ambiguous selection without Q8_0 errors with listing",
                files: &["model-Q4_K_M.gguf", "model-F16.gguf"],
                requested: None,
                expect: ExpectedSelection::Err(&["--file", "model-Q4_K_M.gguf", "model-F16.gguf"]),
            },
            SelectCase {
                name: "no .gguf files errors",
                files: &[],
                requested: None,
                expect: ExpectedSelection::Err(&[]),
            },
        ];

        let failures: Vec<String> = cases
            .iter()
            .filter_map(|case| {
                let siblings: Vec<Sibling> = case.files.iter().map(|f| sibling(f)).collect();
                let refs: Vec<&Sibling> = siblings.iter().collect();
                let result = select_gguf_file(&refs, case.requested);
                match result {
                    Ok(selection) => {
                        // Exhaustive match on the real Selection (hub.rs:169,
                        // Picked/AutoQ8 are its only two variants): a 3rd
                        // variant added there must fail compilation here.
                        let (is_auto_q8, picked) = match selection {
                            Selection::Picked(s) => (false, s),
                            Selection::AutoQ8(s) => (true, s),
                        };
                        match case.expect {
                            ExpectedSelection::Picked(name)
                                if !is_auto_q8 && picked.rfilename == name =>
                            {
                                None
                            }
                            ExpectedSelection::AutoQ8(name)
                                if is_auto_q8 && picked.rfilename == name =>
                            {
                                None
                            }
                            ref other => Some(format!(
                                "{}: expected {other:?}, got {}({:?})",
                                case.name,
                                if is_auto_q8 { "AutoQ8" } else { "Picked" },
                                picked.rfilename
                            )),
                        }
                    }
                    Err(err) => match case.expect {
                        ExpectedSelection::Err(fragments) => {
                            let msg = err.to_string();
                            let missing: Vec<&str> = fragments
                                .iter()
                                .copied()
                                .filter(|f| !msg.contains(f))
                                .collect();
                            (!missing.is_empty()).then(|| {
                                format!(
                                    "{}: error {msg:?} missing fragment(s) {missing:?}",
                                    case.name
                                )
                            })
                        }
                        ref other => {
                            Some(format!("{}: expected {other:?}, got Err({err})", case.name))
                        }
                    },
                }
            })
            .collect();

        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn api_url_percent_encodes_and_builds_the_blobs_query() {
        assert_eq!(
            api_url("https://huggingface.co", "cstr", "parakeet tdt", "main"),
            "https://huggingface.co/api/models/cstr/parakeet%20tdt/revision/main?blobs=true"
        );
    }

    #[test]
    fn resolve_url_cases() {
        for (name, endpoint, rfilename, expect) in [
            (
                "percent-encodes the filename and revision",
                "https://huggingface.co",
                "parakeet tdt Q8_0.gguf",
                "https://huggingface.co/cstr/parakeet-tdt-0.6b-v3-GGUF/resolve/main/parakeet%20tdt%20Q8_0.gguf",
            ),
            (
                "honors a rewritten endpoint",
                "https://mirror.internal.example.com",
                "model.gguf",
                "https://mirror.internal.example.com/cstr/parakeet-tdt-0.6b-v3-GGUF/resolve/main/model.gguf",
            ),
        ] {
            assert_eq!(
                resolve_url(endpoint, "cstr", "parakeet-tdt-0.6b-v3-GGUF", "main", rfilename),
                expect,
                "{name}"
            );
        }
    }

    #[test]
    fn hub_cache_directory_is_revision_specific_and_windows_safe() {
        let models_dir = Path::new("cache/models");
        let main = hub_cache_dir(models_dir, "owner", "repo", "main");
        let main_again = hub_cache_dir(models_dir, "owner", "repo", "main");
        let case_distinct = hub_cache_dir(models_dir, "owner", "repo", "Main");
        let slash_revision = hub_cache_dir(models_dir, "owner", "repo", "refs/pr/1");

        assert_eq!(main, main_again);
        assert_ne!(main, case_distinct);
        assert_ne!(main, slash_revision);
        let component = slash_revision.file_name().unwrap().to_string_lossy();
        assert!(!component.contains(['/', '\\']));
    }

    #[test]
    fn selected_hub_file_names_are_portable() {
        for (name, expect_valid) in [
            ("nested/model-Q8_0.gguf", true),
            ("nested\\model-Q8_0.gguf", true),
            ("nested/CON.gguf", false),
            ("nested/lpt1.model.gguf", false),
            ("nested/..", false),
            ("nested/", false),
        ] {
            assert_eq!(hub_file_name(name).is_ok(), expect_valid, "{name}");
        }
    }
}
