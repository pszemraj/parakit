//! Fetch transaction, download resume, source-lock, and URL file-name regressions.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{mpsc, Arc, Barrier};
use std::time::{Duration, Instant};

fn read_request_headers(stream: &mut impl Read) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..count]);
    }
    request
}

fn serve_model_once(body: &'static [u8]) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request_text = String::from_utf8(read_request_headers(&mut stream)).unwrap();
        let status = if request_text.to_ascii_lowercase().contains("\r\nrange:") {
            "206 Partial Content"
        } else {
            "200 OK"
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nETag: \"revision-2\"\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
        request_text
    });
    (format!("http://{address}/model.gguf"), handle)
}

fn serve_slow_model(body: &'static [u8]) -> (String, std::thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut requests = 0;
        let mut deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && requests < 2 {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(err) => panic!("test server accept failed: {err}"),
            };
            // Windows inherits the listener's nonblocking mode on accept.
            stream.set_nonblocking(false).unwrap();
            requests += 1;

            read_request_headers(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"concurrent\"\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            let split = body.len() / 2;
            stream.write_all(&body[..split]).unwrap();
            stream.flush().unwrap();
            std::thread::sleep(Duration::from_millis(250));
            stream.write_all(&body[split..]).unwrap();
            deadline = Instant::now() + Duration::from_millis(300);
        }
        requests
    });
    (format!("http://{address}/model.gguf"), handle)
}

#[test]
fn move_into_place_replaces_existing_file() {
    let dir = crate::test_support::fixture_root("parakit-fetch-tests", "move-into-place");
    let src = dir.join("model.gguf.part");
    let dst = dir.join("model.gguf");
    std::fs::write(&src, b"new").unwrap();
    std::fs::write(&dst, b"old").unwrap();

    move_into_place(&src, &dst).unwrap();

    assert!(!src.exists());
    assert_eq!(std::fs::read(&dst).unwrap(), b"new");
}

#[test]
fn prepend_env_path_keeps_existing_entries() {
    let dir = Path::new("target/tmp/parakit-fetch-tests/lib");
    let existing = env::join_paths([Path::new("target/tmp/a"), Path::new("target/tmp/b")]).unwrap();
    let joined = joined_path_with_prepended(dir, Some(existing)).unwrap();
    let paths: Vec<_> = env::split_paths(&joined).collect();

    assert_eq!(paths[0], dir);
    assert_eq!(paths[1], Path::new("target/tmp/a"));
    assert_eq!(paths[2], Path::new("target/tmp/b"));
}

#[test]
fn partial_path_uses_the_gguf_part_staging_convention() {
    assert_eq!(
        partial_path(Path::new("/cache/models/hub/o--r/model-Q8_0.gguf")),
        Path::new("/cache/models/hub/o--r/model-Q8_0.gguf.part")
    );
}

#[test]
fn force_discards_a_resumable_partial_without_hashing_the_download() {
    let dir = crate::test_support::fixture_root("parakit-fetch-tests", "force-partial");
    let dest = dir.join("model.gguf");
    let partial = partial_path(&dest);
    std::fs::write(&partial, b"stale-prefix").unwrap();
    let mut validator = partial.as_os_str().to_os_string();
    validator.push(".validator");
    std::fs::write(PathBuf::from(validator), b"\"revision-1\"").unwrap();

    let (url, server) = serve_model_once(b"fresh-model");
    let options = FetchOptions {
        force: true,
        quiet: true,
        verbose: false,
        source: FetchSource::Url {
            url: url.clone(),
            sha256: None,
        },
    };
    let client = Client::builder().no_proxy().build().unwrap();
    download_and_verify(&options, &client, &url, &dest, None, None).unwrap();
    let request = server.join().unwrap();

    assert!(!request.to_ascii_lowercase().contains("\r\nrange:"));
    assert_eq!(std::fs::read(&dest).unwrap(), b"fresh-model");
}

#[test]
fn concurrent_downloads_share_one_destination_transaction() {
    let dir = crate::test_support::fixture_root("parakit-fetch-tests", "concurrent");
    let dest = dir.join("model.gguf");
    let (url, server) = serve_slow_model(b"complete-model");
    let barrier = Arc::new(Barrier::new(3));

    let downloads = (0..2)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let url = url.clone();
            let dest = dest.clone();
            std::thread::spawn(move || {
                let options = FetchOptions {
                    force: false,
                    quiet: true,
                    verbose: false,
                    source: FetchSource::Url {
                        url: url.clone(),
                        sha256: None,
                    },
                };
                let client = Client::builder().no_proxy().build().unwrap();
                barrier.wait();
                download_and_verify(&options, &client, &url, &dest, None, None)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();

    for download in downloads {
        download.join().unwrap().unwrap();
    }
    assert_eq!(server.join().unwrap(), 1);
    assert_eq!(std::fs::read(dest).unwrap(), b"complete-model");
}

#[test]
fn source_rebuild_waits_for_destination_transaction_and_rechecks_cache() {
    let dir = crate::test_support::fixture_root("parakit-fetch-tests", "source-lock");
    let paths = FetchPaths {
        nemo: dir.join(NEMO_FILENAME),
        f16: dir.join(F16_FILENAME),
        q8: dir.join(Q8_FILENAME),
    };
    let held_lock = acquire_artifact_lock(&paths.q8).unwrap();
    let q8 = paths.q8.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();

    let rebuild = std::thread::spawn(move || {
        let options = FetchOptions {
            force: false,
            quiet: true,
            verbose: false,
            source: FetchSource::OfficialNemo {
                keep_nemo: true,
                keep_f16: true,
            },
        };
        started_tx.send(()).unwrap();
        let result = run_official_nemo_with_paths(
            &options,
            "https://example.invalid",
            None,
            true,
            true,
            paths,
        );
        done_tx.send(result).unwrap();
    });

    started_rx.recv().unwrap();
    assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
    std::fs::write(&q8, b"completed-by-first-fetch").unwrap();
    drop(held_lock);

    assert_eq!(
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap(),
        q8
    );
    rebuild.join().unwrap();
}

#[test]
fn explicit_checksum_mismatch_fails_after_one_download() {
    let dir = crate::test_support::fixture_root("parakit-fetch-tests", "sha-mismatch");
    let dest = dir.join("model.gguf");
    let (url, server) = serve_model_once(b"changed-model");
    let options = FetchOptions {
        force: false,
        quiet: true,
        verbose: false,
        source: FetchSource::Url {
            url: url.clone(),
            sha256: Some("0".repeat(64)),
        },
    };
    let client = Client::builder().no_proxy().build().unwrap();

    let error = download_and_verify(&options, &client, &url, &dest, None, Some(&"0".repeat(64)))
        .unwrap_err();
    let request = server.join().unwrap();

    assert!(error.to_string().contains("checksum mismatch"));
    assert!(request.starts_with("GET "));
    assert!(!dest.exists());
}

#[test]
fn url_file_name_cases() {
    for (name, url, expect) in [
        (
            "strips a query string",
            "https://example.com/models/model.gguf?download=true",
            Some("model.gguf"),
        ),
        (
            "strips a fragment",
            "https://example.com/models/model.gguf#frag",
            Some("model.gguf"),
        ),
        (
            "treats a backslash as a Windows path separator",
            "https://example.com/models\\nested\\model.gguf",
            Some("model.gguf"),
        ),
        (
            "rejects a URL with no path segment",
            "https://example.com/",
            None,
        ),
        (
            "rejects a URL whose path ends in a slash",
            "https://example.com/models/",
            None,
        ),
        (
            "rejects a parent-directory segment",
            "https://example.com/models/..",
            None,
        ),
        (
            "rejects a Windows device name with an extension",
            "https://example.com/models/CON.gguf",
            None,
        ),
        (
            "rejects a numbered Windows device name case-insensitively",
            "https://example.com/models/com1.GGUF",
            None,
        ),
        (
            "does not reject a device-name prefix",
            "https://example.com/models/computer.gguf",
            Some("computer.gguf"),
        ),
        (
            "does not reject a device number outside the reserved range",
            "https://example.com/models/LPT10.gguf",
            Some("LPT10.gguf"),
        ),
        (
            "rejects a device name before a second extension",
            "https://example.com/models/prn.model.gguf",
            None,
        ),
        (
            "rejects a device name with a superscript digit",
            "https://example.com/models/lpt².gguf",
            None,
        ),
    ] {
        assert_eq!(url_file_name(url).ok().as_deref(), expect, "{name}");
    }
}

#[test]
fn download_file_names_reject_windows_invalid_characters() {
    for name in [
        "model<variant.gguf",
        "model>variant.gguf",
        "model:stream.gguf",
        "model\"variant.gguf",
        "model/variant.gguf",
        "model\\variant.gguf",
        "model|variant.gguf",
        "model?variant.gguf",
        "model*variant.gguf",
        "model.gguf.",
        "model.gguf ",
        "model\u{1f}.gguf",
    ] {
        assert!(
            validate_download_file_name(name).is_err(),
            "{name:?} should be rejected"
        );
    }
}

#[test]
fn config_model_hint_paths_round_trip_through_toml() {
    for path in [
        r"C:\Users\Name With Space\models\model.gguf",
        "target/models/model \"Q8_0\".gguf",
        "target/models/José/model\ncontinued.gguf",
    ] {
        let literal = toml_path_literal(Path::new(path));
        let parsed: toml::Value = format!("daemon.model = {literal}").parse().unwrap();

        assert_eq!(parsed["daemon"]["model"].as_str(), Some(path), "{path:?}");
    }
}

#[test]
fn url_cache_key_includes_the_exact_url() {
    let models = Path::new("target/tmp/models");
    assert_ne!(
        url_cache_dir(models, "https://one.example/model.gguf"),
        url_cache_dir(models, "https://two.example/model.gguf")
    );
    assert_eq!(
        url_cache_dir(models, "https://one.example/model.gguf"),
        url_cache_dir(models, "https://one.example/model.gguf")
    );
}
