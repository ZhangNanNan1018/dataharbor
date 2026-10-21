use super::*;
use std::net::Shutdown;

fn profile(port: u16) -> StoredProfile {
    serde_json::from_value(serde_json::json!({
        "id": "fixture", "name": "fixture", "engine": "postgres",
        "httpTunnel": true, "httpProxyHost": "127.0.0.1", "httpProxyPort": port
    }))
    .unwrap()
}

fn read_headers(socket: &mut TcpStream) {
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
    }
}

#[test]
fn http_tunnel_serves_two_persistent_connections_and_preserves_fragmented_greeting() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = proxy.local_addr().unwrap().port();
    proxy.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut workers = Vec::new();
        while workers.len() < 2 {
            match proxy.accept() {
                Ok((mut stream, _)) => workers.push(thread::spawn(move || {
                    read_headers(&mut stream);
                    stream.write_all(b"HTTP/1.1 200 Connection").unwrap();
                    thread::sleep(Duration::from_millis(20));
                    stream
                        .write_all(b" established\r\nX-Fixture: yes\r\n\r\nhello")
                        .unwrap();
                    let mut bytes = [0; 4096];
                    loop {
                        match stream.read(&mut bytes) {
                            Ok(0) => break,
                            Ok(size) => stream.write_all(&bytes[..size]).unwrap(),
                            Err(error) => panic!("proxy read: {error}"),
                        }
                    }
                    stream.shutdown(Shutdown::Write).unwrap();
                })),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "second proxy connection was blocked"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("proxy accept: {error}"),
            }
        }
        for worker in workers {
            worker.join().unwrap();
        }
    });
    let tunnel = start_http(&profile(port), "db.example.test", 5432).unwrap();
    let mut first = TcpStream::connect(("127.0.0.1", tunnel.local_port())).unwrap();
    let mut second = TcpStream::connect(("127.0.0.1", tunnel.local_port())).unwrap();
    for stream in [&mut first, &mut second] {
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut greeting = [0; 5];
        stream.read_exact(&mut greeting).unwrap();
        assert_eq!(&greeting, b"hello");
        stream.write_all(b"query").unwrap();
        let mut response = [0; 5];
        stream.read_exact(&mut response).unwrap();
        assert_eq!(&response, b"query");
    }
    // A client half-close must not discard the last database response.
    for stream in [&mut first, &mut second] {
        stream.write_all(b"last").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut tail = Vec::new();
        stream.read_to_end(&mut tail).unwrap();
        assert_eq!(&tail, b"last");
    }
    drop(first);
    drop(second);
    drop(tunnel);
    server.join().unwrap();
}

#[test]
fn dropping_tunnel_cancels_pending_proxy_handshake_and_reaps_worker() {
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = proxy.local_addr().unwrap().port();
    let (accepted, ready) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut socket, _) = proxy.accept().unwrap();
        read_headers(&mut socket);
        accepted.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    let tunnel = start_http(&profile(port), "db.example.test", 5432).unwrap();
    let client = TcpStream::connect(("127.0.0.1", tunnel.local_port())).unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = Instant::now();
    drop(tunnel);
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(client);
    server.join().unwrap();
}

#[test]
fn invalid_proxy_name_is_rejected_during_setup_without_a_background_worker() {
    let mut profile = profile(8080);
    profile.http_proxy_host = "invalid\0proxy".into();
    let result = start_http(&profile, "db.example.test", 5432);
    assert!(matches!(result, Err(error) if error.to_string().contains("无法解析 HTTP 代理")));
}

#[test]
#[ignore = "requires Python with optional paramiko; starts a temporary loopback SSH server"]
fn ssh_multiple_channels_transfer_without_waiting_for_idle_connection() {
    use std::io::BufRead;
    struct Fixture {
        child: std::process::Child,
        directory: std::path::PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    let directory = std::env::temp_dir().join(format!("dataharbor-ssh-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let trust = directory.join("known_hosts");
    let fixture_script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/ssh_forward.py");
    let child = std::process::Command::new("python")
        .arg(fixture_script)
        .arg(&trust)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut fixture = Fixture { child, directory };
    let mut port = String::new();
    std::io::BufReader::new(fixture.child.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    let port: u16 = port
        .trim()
        .parse()
        .expect("SSH fixture failed to start (requires paramiko)");
    let mut profile = profile(0);
    profile.ssh_enabled = true;
    profile.ssh_host = "127.0.0.1".into();
    profile.ssh_port = port;
    profile.ssh_username = "fixture".into();
    let tunnel = start_ssh_with_trust(&profile, "fixture", "echo.fixture", 5432, &trust).unwrap();
    let mut idle = TcpStream::connect(("127.0.0.1", tunnel.local_port())).unwrap();
    idle.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    idle.write_all(b"idle").unwrap();
    let mut acknowledgement = [0; 4];
    idle.read_exact(&mut acknowledgement).unwrap();
    assert_eq!(&acknowledgement, b"idle");
    std::thread::scope(|scope| {
        for index in 0..3 {
            let port = tunnel.local_port();
            scope.spawn(move || {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let expected = (0..262144)
                    .map(|byte| ((byte + index) % 251) as u8)
                    .collect::<Vec<_>>();
                // A separate writer also exercises full-duplex backpressure.
                let mut writer = stream.try_clone().unwrap();
                let payload = expected.clone();
                let sending = thread::spawn(move || {
                    writer.write_all(&payload).unwrap();
                    writer.shutdown(Shutdown::Write).unwrap();
                });
                let mut actual = Vec::new();
                stream.read_to_end(&mut actual).unwrap();
                sending.join().unwrap();
                assert_eq!(actual, expected);
            });
        }
    });
    // Repeated short-lived channels share the still-open idle connection.
    // CHANNEL_CLOSE must be completed without retiring the parent SSH session.
    for _ in 0..50 {
        let mut stream = TcpStream::connect(("127.0.0.1", tunnel.local_port())).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(b"control").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        assert_eq!(&response, b"control");
    }
    let started = Instant::now();
    drop(tunnel);
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(idle);
}
