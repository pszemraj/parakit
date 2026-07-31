//! Hugging Face repo source parsing, Hub API lookup, `.gguf` file selection,
//! and the repo acquisition flow for `parakit fetch <owner>/<repo>`.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::{manifest, net, FetchOptions, FetchSource};

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
enum SourceKind {
    Repo(RepoSpec),
    Url(String),
}

/// Classify a `parakit fetch` positional argument: anything containing
/// `://` is a URL, everything else is parsed as a Hugging Face repo spec.
fn classify_source(input: &str) -> Result<SourceKind> {
    if input.contains("://") {
        return Ok(SourceKind::Url(input.to_string()));
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
    #[serde(default)]
    lfs: Option<LfsInfo>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct LfsInfo {
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiModelResponse {
    #[serde(default)]
    siblings: Vec<Sibling>,
}

/// Normalize an LFS-reported checksum from either `lfs.sha256` or
/// `lfs.oid` (sometimes prefixed `sha256:`), accepting only 64-hex-char
/// results.
fn normalize_lfs_sha256(lfs: Option<&LfsInfo>) -> Option<String> {
    let lfs = lfs?;
    for raw in [lfs.sha256.as_deref(), lfs.oid.as_deref()]
        .into_iter()
        .flatten()
    {
        let stripped = raw.strip_prefix("sha256:").unwrap_or(raw);
        if is_hex64(stripped) {
            return Some(stripped.to_ascii_lowercase());
        }
    }
    None
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_gguf_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".gguf")
}

/// The chosen `.gguf` sibling and why it was picked, distinguished so the
/// caller can print a status line only for the auto-picked case.
#[derive(Debug)]
enum Selection<'a> {
    Explicit(&'a Sibling),
    Only(&'a Sibling),
    AutoQ8(&'a Sibling),
}

impl<'a> Selection<'a> {
    fn sibling(&self) -> &'a Sibling {
        match self {
            Selection::Explicit(s) | Selection::Only(s) | Selection::AutoQ8(s) => s,
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
            .map(|s| Selection::Explicit(s))
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
        return Ok(Selection::Only(gguf_files[0]));
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
/// `models_dir()/hub/<owner>--<repo>/<file>`.
///
/// # Arguments
///
/// * `options` - Shared fetch options (`force`, status verbosity).
/// * `endpoint` - Resolved Hub endpoint (see [`net::resolve_endpoint`]).
/// * `token` - Resolved Hub bearer token, if any.
/// * `repo` - `owner/repo`.
/// * `revision` - Revision to fetch from; `None` defers to `main`.
/// * `file` - Exact `rfilename` to select; `None` triggers automatic selection.
/// * `sha256_override` - `--sha256`, overriding the Hub-reported checksum.
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
    let dest_dir = models_dir.join("hub").join(format!("{owner}--{name}"));
    std::fs::create_dir_all(&dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    let basename = Path::new(&sibling.rfilename)
        .file_name()
        .ok_or_else(|| anyhow!("repo file '{}' has no usable file name", sibling.rfilename))?;
    let dest = dest_dir.join(basename);

    let hub_sha = normalize_lfs_sha256(sibling.lfs.as_ref());
    let expected_sha = sha256_override.map(str::to_string).or(hub_sha);

    if !options.force {
        if let Some(expected) = &expected_sha {
            if dest.is_file() && crate::checksum::sha256_file_hex(&dest)? == *expected {
                options.verbose_status(format_args!(
                    "parakit: cached model is current: {}",
                    dest.display()
                ));
                return Ok(dest);
            }
        }
    }

    let resolve = resolve_url(endpoint, owner, name, revision, &sibling.rfilename);
    let bearer = net::bearer_for(&resolve, endpoint, token);
    options.status(format_args!("parakit: downloading {resolve}"));
    super::download_and_verify(
        options,
        &client,
        &resolve,
        &dest,
        bearer,
        expected_sha.as_deref(),
    )?;

    let relkey = manifest::relative_key(&models_dir, &dest)?;
    let manifest_path = models_dir.join(crate::model::MANIFEST_FILENAME);
    let mut manifest = manifest::Manifest::load(&manifest_path)?.unwrap_or_default();
    manifest.record_download(&relkey, &resolve, expected_sha);
    manifest.save(&manifest_path)?;

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
    fn rejects_garbage_specs() {
        for bad in [
            "",
            "owner",
            "owner/",
            "/repo",
            "a/b/c",
            "owner/repo@",
            "ow ner/repo",
        ] {
            assert!(
                classify_source(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn rejects_invalid_characters_in_owner_or_repo() {
        assert!(parse_repo_spec("ow!ner/repo").is_err());
        assert!(parse_repo_spec("owner/re/po").is_err());
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

    fn sibling(rfilename: &str, lfs: Option<LfsInfo>) -> Sibling {
        Sibling {
            rfilename: rfilename.to_string(),
            lfs,
        }
    }

    #[test]
    fn selects_the_sole_gguf_file() {
        let a = sibling("model-Q8_0.gguf", None);
        let refs = [&a];
        match select_gguf_file(&refs, None).unwrap() {
            Selection::Only(s) => assert_eq!(s.rfilename, "model-Q8_0.gguf"),
            _ => panic!("expected Selection::Only"),
        }
    }

    #[test]
    fn explicit_file_hits_an_exact_match() {
        let a = sibling("model-Q8_0.gguf", None);
        let b = sibling("model-Q4_K_M.gguf", None);
        let refs = [&a, &b];
        match select_gguf_file(&refs, Some("model-Q4_K_M.gguf")).unwrap() {
            Selection::Explicit(s) => assert_eq!(s.rfilename, "model-Q4_K_M.gguf"),
            _ => panic!("expected Selection::Explicit"),
        }
    }

    #[test]
    fn explicit_file_miss_lists_available_files() {
        let a = sibling("model-Q8_0.gguf", None);
        let refs = [&a];
        let err = select_gguf_file(&refs, Some("nope.gguf")).unwrap_err();
        assert!(err.to_string().contains("model-Q8_0.gguf"));
    }

    #[test]
    fn auto_picks_the_sole_q8_0_file_among_several() {
        let a = sibling("model-Q8_0.gguf", None);
        let b = sibling("model-Q4_K_M.gguf", None);
        let c = sibling("model-F16.gguf", None);
        let refs = [&a, &b, &c];
        match select_gguf_file(&refs, None).unwrap() {
            Selection::AutoQ8(s) => assert_eq!(s.rfilename, "model-Q8_0.gguf"),
            _ => panic!("expected Selection::AutoQ8"),
        }
    }

    #[test]
    fn ambiguous_selection_without_q8_0_errors_with_listing() {
        let a = sibling("model-Q4_K_M.gguf", None);
        let b = sibling("model-F16.gguf", None);
        let refs = [&a, &b];
        let err = select_gguf_file(&refs, None).unwrap_err();
        assert!(err.to_string().contains("--file"));
        assert!(err.to_string().contains("model-Q4_K_M.gguf"));
        assert!(err.to_string().contains("model-F16.gguf"));
    }

    #[test]
    fn no_gguf_files_errors() {
        let refs: [&Sibling; 0] = [];
        assert!(select_gguf_file(&refs, None).is_err());
    }

    #[test]
    fn parses_siblings_json_with_oid_prefixed_and_bare_sha256_and_a_non_lfs_file() {
        let json = r#"{
            "siblings": [
                {
                    "rfilename": "model-Q8_0.gguf",
                    "size": 123,
                    "lfs": {
                        "oid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "size": 123
                    }
                },
                {
                    "rfilename": "model-F16.gguf",
                    "lfs": {
                        "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    }
                },
                {
                    "rfilename": "README.md"
                }
            ]
        }"#;
        let parsed: ApiModelResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.siblings.len(), 3);

        let q8 = &parsed.siblings[0];
        assert_eq!(normalize_lfs_sha256(q8.lfs.as_ref()), Some("a".repeat(64)));

        let f16 = &parsed.siblings[1];
        assert_eq!(normalize_lfs_sha256(f16.lfs.as_ref()), Some("b".repeat(64)));

        let readme = &parsed.siblings[2];
        assert!(readme.lfs.is_none());
        assert_eq!(normalize_lfs_sha256(readme.lfs.as_ref()), None);
    }

    #[test]
    fn normalize_lfs_sha256_rejects_non_hex64_values() {
        let lfs = LfsInfo {
            oid: Some("not-a-hash".to_string()),
            sha256: None,
        };
        assert_eq!(normalize_lfs_sha256(Some(&lfs)), None);
    }

    #[test]
    fn api_url_percent_encodes_and_builds_the_blobs_query() {
        assert_eq!(
            api_url("https://huggingface.co", "cstr", "parakeet tdt", "main"),
            "https://huggingface.co/api/models/cstr/parakeet%20tdt/revision/main?blobs=true"
        );
    }

    #[test]
    fn resolve_url_percent_encodes_the_filename_and_revision() {
        assert_eq!(
            resolve_url(
                "https://huggingface.co",
                "cstr",
                "parakeet-tdt-0.6b-v3-GGUF",
                "main",
                "parakeet tdt Q8_0.gguf"
            ),
            "https://huggingface.co/cstr/parakeet-tdt-0.6b-v3-GGUF/resolve/main/parakeet%20tdt%20Q8_0.gguf"
        );
    }

    #[test]
    fn resolve_url_honors_a_rewritten_endpoint() {
        assert_eq!(
            resolve_url(
                "https://mirror.internal.example.com",
                "cstr",
                "parakeet-tdt-0.6b-v3-GGUF",
                "main",
                "model.gguf"
            ),
            "https://mirror.internal.example.com/cstr/parakeet-tdt-0.6b-v3-GGUF/resolve/main/model.gguf"
        );
    }
}
