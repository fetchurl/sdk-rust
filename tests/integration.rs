use std::env;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fetchurl_sdk as fetchurl;
use testcontainers::clients::Cli;
use testcontainers::core::WaitFor;
use testcontainers::{GenericImage, RunnableImage};

/// Restores `FETCHURL_SERVER` when dropped (including on panic).
struct FetchurlServerEnv(Option<String>);

impl FetchurlServerEnv {
    fn set(value: String) -> Self {
        let previous = env::var("FETCHURL_SERVER").ok();
        unsafe {
            env::set_var("FETCHURL_SERVER", &value);
        }
        Self(previous)
    }
}

impl Drop for FetchurlServerEnv {
    fn drop(&mut self) {
        match self.0.take() {
            Some(val) => unsafe {
                env::set_var("FETCHURL_SERVER", val);
            },
            None => unsafe {
                env::remove_var("FETCHURL_SERVER");
            },
        }
    }
}

fn parse_image(image: &str) -> (String, String) {
    if let Some((name, tag)) = image.rsplit_once(':') {
        if !tag.contains('/') {
            return (name.to_string(), tag.to_string());
        }
    }
    (image.to_string(), "latest".to_string())
}

/// Unique temp dir so parallel test processes do not clobber each other.
fn write_temp_file(content: &[u8]) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "fetchurl-test-upstream-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("file");
    fs::write(&path, content).expect("write file");
    dir
}

/// Poll until `url` returns HTTP 200 or `deadline` is hit.
///
/// Prefer this over fixed `WaitFor::seconds` sleeps and over log scraping:
/// Python's http.server may fully-buffer stdout when not on a TTY, so
/// "Serving HTTP" never appears in docker logs until much later.
///
/// Require 200 (not merely "accept()"), so a mid-boot 502/404 does not count
/// as ready. Spec: health is healthy only on status 200; the upstream probe
/// hits the real object path so a missing volume mount fails before the session.
fn wait_http_200(agent: &ureq::Agent, url: &str, deadline: Instant) {
    while Instant::now() < deadline {
        match agent.get(url).call() {
            Ok(resp) if resp.status() == 200 => return,
            Ok(_) | Err(ureq::Error::Status(_, _)) => {
                // Listener is up but not ready (or wrong path) — keep polling.
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    panic!("timed out waiting for HTTP 200 from {url}");
}

#[test]
fn integration_fetchurl_server() {
    let image = match env::var("FETCHURL_TEST_IMAGE") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            eprintln!("FETCHURL_TEST_IMAGE not set; skipping integration test");
            return;
        }
    };

    let content = b"integration-test".to_vec();
    let hash = {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&content);
        format!("{:x}", hasher.finalize())
    };

    let (name, tag) = parse_image(&image);
    let docker = Cli::default();
    let network_name = format!("fetchurl-test-net-{}", std::process::id());
    let upstream_name = format!("upstream-{}", std::process::id());

    let upstream_dir = write_temp_file(&content);
    // Clean up unique upstream dir when the test finishes.
    struct Rm(PathBuf);
    impl Drop for Rm {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _upstream_cleanup = Rm(upstream_dir.clone());

    // Do not rely on fixed sleeps or log lines for readiness (see wait_reachable).
    let upstream_image = GenericImage::new("python", "3.12-alpine")
        .with_volume(
            upstream_dir.to_string_lossy().to_string(),
            "/srv".to_string(),
        )
        .with_exposed_port(8000)
        .with_wait_for(WaitFor::Nothing);
    let upstream = docker.run(
        RunnableImage::from((
            upstream_image,
            vec![
                "python".to_string(),
                "-m".to_string(),
                "http.server".to_string(),
                "8000".to_string(),
                "--bind".to_string(),
                "0.0.0.0".to_string(),
                "--directory".to_string(),
                "/srv".to_string(),
            ],
        ))
        .with_network(network_name.clone())
        .with_container_name(upstream_name.as_str()),
    );

    let server_image = GenericImage::new(name, tag)
        .with_exposed_port(8080)
        .with_env_var("FETCHURL_ALLOW_PRIVATE_IPS", "1")
        .with_wait_for(WaitFor::Nothing);
    let server = docker.run(
        RunnableImage::from((server_image, vec!["server".to_string()])).with_network(network_name),
    );

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .build();

    let deadline = Instant::now() + Duration::from_secs(45);

    // Host-mapped ports: poll until each probe path is actually healthy (HTTP 200).
    let upstream_port = upstream.get_host_port_ipv4(8000);
    wait_http_200(
        &agent,
        &format!("http://127.0.0.1:{upstream_port}/file"),
        deadline,
    );

    let host_port = server.get_host_port_ipv4(8080);
    // Reference server exposes /api/fetchurl/health; 200 means healthy per spec.
    wait_http_200(
        &agent,
        &format!("http://127.0.0.1:{host_port}/api/fetchurl/health"),
        deadline,
    );

    let _env_guard =
        FetchurlServerEnv::set(format!("\"http://127.0.0.1:{host_port}/api/fetchurl\""));

    let source_url = format!("http://{upstream_name}:8000/file");
    let mut session = fetchurl::FetchSession::new("sha256", &hash, &[source_url.as_str()]).unwrap();

    let mut output = Vec::new();
    while let Some(attempt) = session.next_attempt() {
        if Instant::now() > deadline {
            panic!("integration test timed out");
        }
        let mut req = agent.get(attempt.url());
        for (k, v) in attempt.headers() {
            req = req.set(k, v);
        }
        let resp = match req.call() {
            Ok(resp) => resp,
            Err(_) => continue,
        };
        if resp.status() != 200 {
            continue;
        }

        // Fresh buffer per attempt so a failed body cannot taint the next try.
        let mut attempt_out = Vec::new();
        let mut verifier = session.verifier(&mut attempt_out);
        let mut reader = resp.into_reader();
        let mut buf = [0u8; 8192];
        let mut read_err = false;
        loop {
            if Instant::now() > deadline {
                panic!("integration test timed out");
            }
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if verifier.write_all(&buf[..n]).is_err() {
                        read_err = true;
                        break;
                    }
                }
                Err(_) => {
                    read_err = true;
                    break;
                }
            }
        }

        if read_err {
            if verifier.bytes_written() > 0 {
                session.report_partial();
                break;
            }
            continue;
        }

        match verifier.finish() {
            Ok(_) => {
                output = attempt_out;
                session.report_success();
                break;
            }
            Err(_) => {
                if !attempt_out.is_empty() {
                    session.report_partial();
                    break;
                }
                // Empty body + hash mismatch: try next source.
            }
        }
    }

    // Drop containers before asserts so Docker resources free even if asserts fail.
    drop(server);
    drop(upstream);

    assert_eq!(output, content);
    assert!(session.succeeded());
}
