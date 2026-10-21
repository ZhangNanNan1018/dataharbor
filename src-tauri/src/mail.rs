use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use native_tls::TlsConnector;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct MailRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub attachment_name: String,
    pub attachment: Vec<u8>,
    pub starttls: bool,
}

pub fn format_message(request: &MailRequest) -> String {
    let boundary = "dataharbor-mail";
    let attachment = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        &request.attachment,
    );
    let mut wrapped = String::new();
    for (index, chunk) in attachment.as_bytes().chunks(76).enumerate() {
        if index > 0 {
            wrapped.push_str("\r\n");
        }
        wrapped.push_str(std::str::from_utf8(chunk).unwrap_or(""));
    }
    format!(
        "From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary={boundary}\r\n\r\n--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{body}\r\n--{boundary}\r\nContent-Type: application/octet-stream; name=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"{name}\"\r\n\r\n{wrapped}\r\n--{boundary}--\r\n",
        from = request.from,
        to = request.to,
        subject = request.subject,
        body = request.body,
        name = request.attachment_name,
    )
}

pub fn send_mail(request: &MailRequest) -> AppResult<()> {
    for text in [
        &request.from,
        &request.to,
        &request.subject,
        &request.attachment_name,
    ] {
        if text.contains(['\r', '\n']) {
            return Err(AppError::msg("邮件地址、主题和附件名称不能包含换行"));
        }
    }
    let message = format_message(request);
    let tcp = connect_smtp(&request.host, request.port)?;
    tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(15))).ok();
    if request.port == 465 {
        let connector = TlsConnector::new().map_err(|err| AppError::msg(err.to_string()))?;
        let stream = connector
            .connect(&request.host, tcp)
            .map_err(|err| AppError::msg(format!("SMTP TLS 失败: {err}")))?;
        talk(stream, request, &message)
    } else {
        let mut stream = tcp;
        expect_code(&mut stream, &[220])?;
        write_line(&mut stream, "EHLO dataharbor")?;
        expect_code(&mut stream, &[250])?;
        if request.starttls {
            write_line(&mut stream, "STARTTLS")?;
            expect_code(&mut stream, &[220])?;
            let connector = TlsConnector::new().map_err(|err| AppError::msg(err.to_string()))?;
            let tls = connector
                .connect(&request.host, stream)
                .map_err(|err| AppError::msg(format!("STARTTLS 失败: {err}")))?;
            return talk_after_tls(tls, request, &message);
        }
        talk_authed(stream, request, &message)
    }
}

fn connect_smtp(host: &str, port: u16) -> AppResult<TcpStream> {
    let started = Instant::now();
    let budget = Duration::from_secs(10);
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|err| AppError::msg(format!("SMTP 地址无法解析：{err}")))?;
    let mut failure = "没有可用地址".to_string();
    for address in addresses {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(stream) => return Ok(stream),
            Err(err) => failure = err.to_string(),
        }
    }
    Err(AppError::msg(format!("无法连接 SMTP：{failure}")))
}

fn talk<S: Read + Write>(mut stream: S, request: &MailRequest, message: &str) -> AppResult<()> {
    expect_code(&mut stream, &[220])?;
    write_line(&mut stream, "EHLO dataharbor")?;
    expect_code(&mut stream, &[250])?;
    talk_authed(stream, request, message)
}

fn talk_after_tls<S: Read + Write>(
    mut stream: S,
    request: &MailRequest,
    message: &str,
) -> AppResult<()> {
    write_line(&mut stream, "EHLO dataharbor")?;
    expect_code(&mut stream, &[250])?;
    talk_authed(stream, request, message)
}

fn talk_authed<S: Read + Write>(
    mut stream: S,
    request: &MailRequest,
    message: &str,
) -> AppResult<()> {
    if !request.username.is_empty() {
        write_line(&mut stream, "AUTH LOGIN")?;
        expect_code(&mut stream, &[334])?;
        let user = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            request.username.as_bytes(),
        );
        let pass = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            request.password.as_bytes(),
        );
        write_line(&mut stream, &user)?;
        expect_code(&mut stream, &[334])?;
        write_line(&mut stream, &pass)?;
        expect_code(&mut stream, &[235])?;
    }
    write_line(&mut stream, &format!("MAIL FROM:<{}>", request.from))?;
    expect_code(&mut stream, &[250])?;
    write_line(&mut stream, &format!("RCPT TO:<{}>", request.to))?;
    expect_code(&mut stream, &[250, 251])?;
    write_line(&mut stream, "DATA")?;
    expect_code(&mut stream, &[354])?;
    let dotted = message.replace("\r\n.", "\r\n..");
    stream
        .write_all(format!("{dotted}\r\n.\r\n").as_bytes())
        .map_err(|err| AppError::msg(err.to_string()))?;
    expect_code(&mut stream, &[250])?;
    write_line(&mut stream, "QUIT")?;
    Ok(())
}

fn expect_code<S: Read>(stream: &mut S, codes: &[u16]) -> AppResult<()> {
    let reply = read_reply(stream)?;
    if reply
        .get(..3)
        .and_then(|value| value.parse::<u16>().ok())
        .is_some_and(|code| codes.contains(&code))
    {
        Ok(())
    } else {
        Err(AppError::msg(reply))
    }
}

fn write_line<S: Write>(stream: &mut S, line: &str) -> AppResult<()> {
    stream
        .write_all(format!("{line}\r\n").as_bytes())
        .map_err(|err| AppError::msg(err.to_string()))
}

fn read_reply<S: Read>(stream: &mut S) -> AppResult<String> {
    let mut reply = String::new();
    let mut expected = None;
    loop {
        let mut line = Vec::new();
        loop {
            let mut byte = [0u8];
            stream
                .read_exact(&mut byte)
                .map_err(|err| AppError::msg(format!("SMTP 响应不完整：{err}")))?;
            line.push(byte[0]);
            if line.len() > 4096 || reply.len() + line.len() > 64 * 1024 {
                return Err(AppError::msg("SMTP 响应过长"));
            }
            if line.ends_with(b"\r\n") {
                break;
            }
        }
        if line.len() < 6
            || !line[..3].iter().all(u8::is_ascii_digit)
            || !matches!(line[3], b' ' | b'-')
        {
            return Err(AppError::msg("SMTP 响应格式无效"));
        }
        let code = [line[0], line[1], line[2]];
        if expected.is_some_and(|expected| expected != code) {
            return Err(AppError::msg("SMTP 多行响应状态码不一致"));
        }
        expected = Some(code);
        reply.push_str(&String::from_utf8_lossy(&line));
        if line[3] == b' ' {
            return Ok(reply.trim_end().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_contains_subject_and_attachment() {
        let text = format_message(&MailRequest {
            host: "localhost".into(),
            port: 25,
            username: String::new(),
            password: String::new(),
            from: "a@example.com".into(),
            to: "b@example.com".into(),
            subject: "备份完成".into(),
            body: "见附件".into(),
            attachment_name: "backup.sql".into(),
            attachment: b"SELECT 1;".to_vec(),
            starttls: false,
        });
        assert!(text.contains("Subject: 备份完成"));
        assert!(text.contains("filename=\"backup.sql\""));
        assert!(text.contains("U0VMRUNUIDE7"));
    }

    #[test]
    fn smtp_hostname_resolves_to_loopback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = connect_smtp("localhost", listener.local_addr().unwrap().port()).unwrap();
        let (_, address) = listener.accept().unwrap();
        assert!(address.ip().is_loopback());
        drop(client);
    }

    #[test]
    fn fragmented_multiline_reply_does_not_consume_next_reply() {
        struct Fragmented(std::io::Cursor<Vec<u8>>);
        impl Read for Fragmented {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let length = buffer.len().min(1);
                self.0.read(&mut buffer[..length])
            }
        }
        let mut stream = Fragmented(std::io::Cursor::new(
            b"250-host\r\n250-AUTH LOGIN\r\n250 STARTTLS\r\n334 challenge\r\n".to_vec(),
        ));
        let hello = read_reply(&mut stream).unwrap();
        assert!(hello.contains("250-AUTH LOGIN"));
        assert!(hello.ends_with("250 STARTTLS"));
        expect_code(&mut stream, &[334]).unwrap();
        assert!(read_reply(&mut std::io::Cursor::new(b"250-truncated")).is_err());
        assert!(read_reply(&mut std::io::Cursor::new(b"250-first\r\n550 wrong\r\n")).is_err());
    }

    #[test]
    fn rejected_auth_challenge_stops_before_sending_credentials() {
        struct Script {
            read: std::io::Cursor<Vec<u8>>,
            written: Vec<u8>,
        }
        impl Read for Script {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.read.read(buffer)
            }
        }
        impl Write for Script {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.written.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut stream = Script {
            read: std::io::Cursor::new(b"535 no auth\r\n".to_vec()),
            written: Vec::new(),
        };
        let request = MailRequest {
            host: "localhost".into(),
            port: 25,
            username: "user".into(),
            password: "secret".into(),
            from: "a@example.test".into(),
            to: "b@example.test".into(),
            subject: String::new(),
            body: String::new(),
            attachment_name: String::new(),
            attachment: Vec::new(),
            starttls: false,
        };
        assert!(talk_authed(&mut stream, &request, "message").is_err());
        assert_eq!(stream.written, b"AUTH LOGIN\r\n");
        assert!(expect_code(&mut std::io::Cursor::new(b"354 wrong stage\r\n"), &[250]).is_err());
    }
}
