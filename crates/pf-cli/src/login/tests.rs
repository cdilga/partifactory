use super::*;
use std::{
    io::Read,
    net::TcpListener,
    sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    },
};

#[test]
fn persisted_bundle_is_private_and_rejects_links() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new/config.json");
    let saved = json!({"url":"https://parts.example.com","token":"secret","refresh_token":"refresh","expires_at":123});
    write_config(&path, &saved).unwrap();
    assert_eq!(read_config(&path).unwrap(), saved);
    write_config(&path, &json!({"token":"rotated"})).unwrap();
    assert_eq!(read_config(&path).unwrap()["token"], "rotated");
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_config(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(dir.path().join("missing"), &path).unwrap();
        assert!(read_config(&path).is_err());
        assert!(write_config(&path, &saved).is_err());
        let lock_path = path.with_extension("lock");
        std::os::unix::fs::symlink(dir.path().join("missing-lock-target"), &lock_path).unwrap();
        assert!(lock(&path).is_err());
        assert!(!dir.path().join("missing-lock-target").exists());
    }
}

#[test]
fn validates_workspace_origin_and_sanitizes_oauth_errors() {
    assert_eq!(
        validated_url("https://PARTS.example.com:443/").unwrap(),
        "https://parts.example.com"
    );
    assert!(validated_url("http://127.0.0.1:1234").is_ok());
    for bad in [
        "http://parts.example.com",
        "https://user:secret@parts.example.com",
        "https://parts.example.com/path",
        "https://parts.example.com/?token=secret",
        "https://parts.example.com/#secret",
    ] {
        assert!(validated_url(bad).is_err());
    }
    let failure =
        oauth_failure(&json!({"error":"invalid_grant","error_description":"pf_rt_secret"}));
    assert!(!failure.body.to_string().contains("pf_rt_secret"));
    assert!(
        bundle(
            "https://parts.example.com",
            &json!({"access_token":"secret","refresh_token":"refresh","expires_in":3601})
        )
        .is_err()
    );
}

#[test]
fn concurrent_refresh_exchanges_single_use_token_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    write_config(
        &path,
        &json!({"url":url,"token":"expired","refresh_token":"one-use","expires_at":0}),
    )
    .unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    let expected_resource = format!("{url}/");
    let server = std::thread::spawn(move || {
        // Wait up to 20 s for the first request (slow under a loaded test run), then watch for
        // 2 s more: a second refresh exchange in that window is the bug this test catches.
        let mut deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut seen = false;
        while std::time::Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => {
                    if !seen {
                        seen = true;
                        deadline = std::time::Instant::now() + Duration::from_secs(2);
                    }
                    connection
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(err) => panic!("accept failed: {err}"),
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            let (header_end, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..end]).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while request.len() < header_end + length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
            }
            let body = std::str::from_utf8(&request[header_end..header_end + length]).unwrap();
            let form = reqwest::Url::parse(&format!("http://localhost/?{body}"))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect::<std::collections::BTreeMap<_, _>>();
            assert_eq!(form.get("grant_type").unwrap(), "refresh_token");
            assert_eq!(form.get("refresh_token").unwrap(), "one-use");
            assert_eq!(form.get("resource").unwrap(), &expected_resource);
            count.fetch_add(1, Ordering::SeqCst);
            let body = r#"{"access_token":"fresh","refresh_token":"rotated","expires_in":3600}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let barrier = Arc::new(Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let path = path.clone();
            let url = url.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let http = reqwest::blocking::Client::new();
                barrier.wait();
                refresh_locked(&path, &url, &http).unwrap()
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), "fresh");
    }
    server.join().unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    let persisted = read_config(&path).unwrap();
    assert_eq!(persisted["refresh_token"], "rotated");
    assert_eq!(persisted["token"], "fresh");
}
