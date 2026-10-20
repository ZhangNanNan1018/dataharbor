use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ssh2::{CheckResult, KnownHostFileKind, Session};

use crate::error::{AppError, AppResult};
use crate::models::StoredProfile;

pub struct TunnelGuard {
    shutdown: Arc<AtomicBool>,
    local_port: u16,
    thread: Option<JoinHandle<()>>,
}

impl TunnelGuard {
    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for TunnelGuard {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", self.local_port).parse().unwrap(),
            Duration::from_millis(200),
        );
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

pub fn start(profile: &StoredProfile, ssh_password: &str, target_host: &str, target_port: u16) -> AppResult<Option<TunnelGuard>> {
    if profile.ssh_enabled {
        return Ok(Some(start_ssh(profile, ssh_password, target_host, target_port)?));
    }
    if profile.http_tunnel {
        return Ok(Some(start_http(profile, target_host, target_port)?));
    }
    Ok(None)
}

fn start_ssh(
    profile: &StoredProfile,
    password: &str,
    target_host: &str,
    target_port: u16,
) -> AppResult<TunnelGuard> {
    if profile.ssh_host.trim().is_empty() || profile.ssh_username.trim().is_empty() {
        return Err(AppError::msg("启用 SSH 时需要填写跳板机主机和用户"));
    }
    let host = normalized_host(&profile.ssh_host);
    let port = if profile.ssh_port == 0 { 22 } else { profile.ssh_port };
    let tcp = connect_endpoint(host, port)
        .map_err(|err| AppError::msg(format!("无法连接 SSH {}: {err}", endpoint(host, port))))?;
    tcp.set_read_timeout(Some(Duration::from_secs(20))).ok();
    let mut session = Session::new().map_err(|err| AppError::msg(err.to_string()))?;
    session.set_tcp_stream(tcp);
    session.set_timeout(20_000);
    session.handshake().map_err(|err| AppError::msg(format!("SSH 握手失败: {err}")))?;
    let known_hosts_path = dirs::home_dir()
        .ok_or_else(|| AppError::msg("无法定位用户目录，不能验证 SSH 主机身份"))?
        .join(".ssh/known_hosts");
    let (host_key, _) = session.host_key().ok_or_else(|| AppError::msg("SSH 服务器未提供主机密钥，已拒绝认证"))?;
    verify_host_key(&session, host, port, host_key, &known_hosts_path)?;
    if !profile.ssh_key_path.trim().is_empty() {
        session
            .userauth_pubkey_file(
                profile.ssh_username.trim(),
                None,
                std::path::Path::new(profile.ssh_key_path.trim()),
                Some(password),
            )
            .map_err(|err| AppError::msg(format!("SSH 公钥登录失败: {err}")))?;
    } else {
        if password.is_empty() {
            return Err(AppError::msg("启用 SSH 密码登录时需要填写 SSH 密码"));
        }
        session
            .userauth_password(profile.ssh_username.trim(), password)
            .map_err(|err| AppError::msg(format!("SSH 登录失败: {err}")))?;
    }
    if !session.authenticated() {
        return Err(AppError::msg("SSH 未通过认证"));
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| AppError::msg(err.to_string()))?;
    let local_port = listener.local_addr().map_err(|err| AppError::msg(err.to_string()))?.port();
    listener.set_nonblocking(true).map_err(|err| AppError::msg(err.to_string()))?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = shutdown.clone();
    let target_host = normalized_host(target_host).to_string();
    let thread = thread::spawn(move || ssh_loop(session, listener, flag, target_host, target_port));
    Ok(TunnelGuard {
        shutdown,
        local_port,
        thread: Some(thread),
    })
}

fn normalized_host(host: &str) -> &str {
    let host = host.trim();
    host.strip_prefix('[').and_then(|item| item.strip_suffix(']')).unwrap_or(host)
}

fn endpoint(host: &str, port: u16) -> String {
    let host = normalized_host(host);
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

fn connect_endpoint(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let addresses = (normalized_host(host), port).to_socket_addrs()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last_error = std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "地址未解析到 IPv4 或 IPv6");
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { break; }
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = err,
        }
    }
    Err(last_error)
}

fn verify_host_key(session: &Session, host: &str, port: u16, key: &[u8], path: &Path) -> AppResult<()> {
    let mut known_hosts = session.known_hosts().map_err(|err| AppError::msg(format!("无法初始化 SSH 主机校验: {err}")))?;
    known_hosts.read_file(path, KnownHostFileKind::OpenSSH).map_err(|err| {
        AppError::msg(format!("无法读取 SSH 信任文件 {}: {err}。请先通过独立可信渠道核对主机指纹，再使用 OpenSSH 记录主机；DataHarbor 不会自动信任或写入主机密钥。", path.display()))
    })?;
    // OpenSSH uses a plain host only for port 22. Check the exact name so a
    // trusted service on one port cannot silently authorize another service.
    let name = if port == 22 { normalized_host(host).to_string() } else { format!("[{}]:{port}", normalized_host(host)) };
    match known_hosts.check(&name, key) {
        CheckResult::Match => Ok(()),
        CheckResult::NotFound => Err(AppError::msg(format!("SSH 主机 {} 尚未在 {} 中受信任，已在发送登录凭据前拒绝连接。请先独立核对指纹并使用 OpenSSH 确认该主机和端口。", endpoint(host, port), path.display()))),
        CheckResult::Mismatch => Err(AppError::msg(format!("SSH 主机 {} 的密钥与 {} 不一致，已拒绝连接。请联系管理员核对变更，勿直接删除或覆盖旧记录。", endpoint(host, port), path.display()))),
        CheckResult::Failure => Err(AppError::msg(format!("无法验证 SSH 主机 {}，已拒绝连接；请检查 {} 的格式。", endpoint(host, port), path.display()))),
    }
}

fn ssh_loop(session: Session, listener: TcpListener, shutdown: Arc<AtomicBool>, target_host: String, target_port: u16) {
    session.set_blocking(false);
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((client, _)) => {
                if let Ok(channel) = session.channel_direct_tcpip(&target_host, target_port, None) {
                    splice(client, channel, &shutdown);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(30));
            }
            Err(_) => break,
        }
    }
}

fn start_http(profile: &StoredProfile, target_host: &str, target_port: u16) -> AppResult<TunnelGuard> {
    if profile.http_proxy_host.trim().is_empty() {
        return Err(AppError::msg("启用 HTTP 隧道时需要填写代理主机"));
    }
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| AppError::msg(err.to_string()))?;
    let local_port = listener.local_addr().map_err(|err| AppError::msg(err.to_string()))?.port();
    listener.set_nonblocking(true).ok();
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = shutdown.clone();
    let proxy_host = profile.http_proxy_host.trim().to_string();
    let proxy_port = if profile.http_proxy_port == 0 { 8080 } else { profile.http_proxy_port };
    let target_host = normalized_host(target_host).to_string();
    let thread = thread::spawn(move || http_loop(listener, flag, proxy_host, proxy_port, target_host, target_port));
    Ok(TunnelGuard {
        shutdown,
        local_port,
        thread: Some(thread),
    })
}

fn http_loop(listener: TcpListener, shutdown: Arc<AtomicBool>, proxy_host: String, proxy_port: u16, target_host: String, target_port: u16) {
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((client, _)) => {
                if let Ok(remote) = connect_proxy(&proxy_host, proxy_port, &target_host, target_port) {
                    splice_tcp(client, remote, &shutdown);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(30)),
            Err(_) => break,
        }
    }
}

fn connect_proxy(proxy_host: &str, proxy_port: u16, target_host: &str, target_port: u16) -> std::io::Result<TcpStream> {
    let mut stream = connect_endpoint(proxy_host, proxy_port)?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    let target = endpoint(target_host, target_port);
    let request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\nProxy-Connection: keep-alive\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf)?;
    let header = String::from_utf8_lossy(&buf[..n]);
    if !header.starts_with("HTTP/1.1 200") && !header.starts_with("HTTP/1.0 200") {
        return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("HTTP 隧道被拒绝: {}", header.lines().next().unwrap_or("无响应"))));
    }
    let _ = stream.set_nonblocking(true);
    Ok(stream)
}

fn splice(mut client: TcpStream, mut channel: ssh2::Channel, shutdown: &AtomicBool) {
    let _ = client.set_nonblocking(true);
    let mut left = [0u8; 16 * 1024];
    let mut right = [0u8; 16 * 1024];
    while !shutdown.load(Ordering::Relaxed) {
        let mut progress = false;
        match client.read(&mut left) {
            Ok(0) => break,
            Ok(n) => {
                if channel.write_all(&left[..n]).is_err() {
                    break;
                }
                progress = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        match channel.read(&mut right) {
            Ok(0) => break,
            Ok(n) => {
                if client.write_all(&right[..n]).is_err() {
                    break;
                }
                progress = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        if !progress {
            thread::sleep(Duration::from_millis(8));
        }
    }
}

fn splice_tcp(mut client: TcpStream, mut remote: TcpStream, shutdown: &AtomicBool) {
    let _ = client.set_nonblocking(true);
    let _ = remote.set_nonblocking(true);
    let mut left = [0u8; 16 * 1024];
    let mut right = [0u8; 16 * 1024];
    while !shutdown.load(Ordering::Relaxed) {
        let mut progress = false;
        match client.read(&mut left) {
            Ok(0) => break,
            Ok(n) => {
                if remote.write_all(&left[..n]).is_err() {
                    break;
                }
                progress = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        match remote.read(&mut right) {
            Ok(0) => break,
            Ok(n) => {
                if client.write_all(&right[..n]).is_err() {
                    break;
                }
                progress = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        if !progress {
            thread::sleep(Duration::from_millis(8));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    struct Fixture {
        directory: std::path::PathBuf,
        path: std::path::PathBuf,
        key: Vec<u8>,
    }

    impl Fixture {
        fn new(host: &str) -> Self {
            let directory = std::env::temp_dir().join(format!("dataharbor-knownhosts-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&directory).unwrap();
            let path = directory.join("known_hosts");
            // A dummy public Ed25519 key, containing no private key material.
            let mut key = b"\0\0\0\x0bssh-ed25519\0\0\0\x20".to_vec();
            key.extend_from_slice(&[7u8; 32]);
            std::fs::write(&path, format!("{host} ssh-ed25519 {} fixture\n", STANDARD.encode(&key))).unwrap();
            Self { directory, path, key }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn trusted_host_matches_and_unknown_or_changed_key_is_refused() {
        let fixture = Fixture::new("jump.example.test");
        let session = Session::new().unwrap();
        assert!(verify_host_key(&session, "jump.example.test", 22, &fixture.key, &fixture.path).is_ok());
        let unknown = verify_host_key(&session, "other.example.test", 22, &fixture.key, &fixture.path).unwrap_err();
        assert!(unknown.to_string().contains("尚未"));
        let mut changed_key = fixture.key.clone();
        *changed_key.last_mut().unwrap() = 8;
        let changed = verify_host_key(&session, "jump.example.test", 22, &changed_key, &fixture.path).unwrap_err();
        assert!(changed.to_string().contains("不一致"));
    }

    #[test]
    fn known_host_trust_is_scoped_to_the_exact_port() {
        let fixture = Fixture::new("[jump.example.test]:2222");
        let session = Session::new().unwrap();
        assert!(verify_host_key(&session, "jump.example.test", 2222, &fixture.key, &fixture.path).is_ok());
        assert!(verify_host_key(&session, "jump.example.test", 22, &fixture.key, &fixture.path).is_err());
        assert!(verify_host_key(&session, "jump.example.test", 2223, &fixture.key, &fixture.path).is_err());
        let default_port = Fixture::new("jump.example.test");
        assert!(verify_host_key(&session, "jump.example.test", 2222, &default_port.key, &default_port.path).is_err());
    }

    #[test]
    fn ipv6_known_host_and_endpoint_are_not_confused_with_a_port() {
        let fixture = Fixture::new("[2001:db8::1]:2222");
        let session = Session::new().unwrap();
        assert!(verify_host_key(&session, "[2001:db8::1]", 2222, &fixture.key, &fixture.path).is_ok());
        assert_eq!(endpoint("2001:db8::1", 443), "[2001:db8::1]:443");
        assert_eq!(endpoint("[2001:db8::1]", 443), "[2001:db8::1]:443");
        assert_eq!(endpoint("jump.example.test", 2222), "jump.example.test:2222");
    }

    #[test]
    fn missing_or_malformed_trust_file_never_authorizes_a_key() {
        let fixture = Fixture::new("jump.example.test");
        let session = Session::new().unwrap();
        let missing = verify_host_key(&session, "jump.example.test", 22, &fixture.key, &fixture.directory.join("missing")).unwrap_err();
        assert!(missing.to_string().contains("无法读取"));
        std::fs::write(&fixture.path, "invalid known-host record\n").unwrap();
        assert!(verify_host_key(&session, "jump.example.test", 22, &fixture.key, &fixture.path).is_err());
    }
}
