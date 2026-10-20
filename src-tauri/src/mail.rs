use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

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
    let attachment = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &request.attachment);
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
    let message = format_message(request);
    let addr = format!("{}:{}", request.host, request.port);
    let tcp = TcpStream::connect_timeout(
        &addr.parse().map_err(|err| AppError::msg(format!("SMTP 地址无效: {err}")))?,
        Duration::from_secs(10),
    )
    .map_err(|err| AppError::msg(format!("无法连接 SMTP: {err}")))?;
    tcp.set_read_timeout(Some(Duration::from_secs(15))).ok();
    if request.port == 465 {
        let connector = TlsConnector::new().map_err(|err| AppError::msg(err.to_string()))?;
        let stream = connector
            .connect(&request.host, tcp)
            .map_err(|err| AppError::msg(format!("SMTP TLS 失败: {err}")))?;
        talk(stream, request, &message)
    } else {
        let mut stream = tcp;
        let greeting = read_reply(&mut stream)?;
        if !greeting.starts_with("2") {
            return Err(AppError::msg(greeting));
        }
        write_line(&mut stream, &format!("EHLO dataharbor"))?;
        let hello = read_reply(&mut stream)?;
        if request.starttls {
            write_line(&mut stream, "STARTTLS")?;
            let ready = read_reply(&mut stream)?;
            if !ready.starts_with("2") {
                return Err(AppError::msg(ready));
            }
            let connector = TlsConnector::new().map_err(|err| AppError::msg(err.to_string()))?;
            let tls = connector
                .connect(&request.host, stream)
                .map_err(|err| AppError::msg(format!("STARTTLS 失败: {err}")))?;
            return talk_after_tls(tls, request, &message, &hello);
        }
        talk_authed(stream, request, &message)
    }
}

fn talk<S: Read + Write>(mut stream: S, request: &MailRequest, message: &str) -> AppResult<()> {
    let greeting = read_reply(&mut stream)?;
    if !greeting.starts_with("2") {
        return Err(AppError::msg(greeting));
    }
    write_line(&mut stream, "EHLO dataharbor")?;
    let _ = read_reply(&mut stream)?;
    talk_authed(stream, request, message)
}

fn talk_after_tls<S: Read + Write>(mut stream: S, request: &MailRequest, message: &str, _hello: &str) -> AppResult<()> {
    write_line(&mut stream, "EHLO dataharbor")?;
    let _ = read_reply(&mut stream)?;
    talk_authed(stream, request, message)
}

fn talk_authed<S: Read + Write>(mut stream: S, request: &MailRequest, message: &str) -> AppResult<()> {
    if !request.username.is_empty() {
        write_line(&mut stream, "AUTH LOGIN")?;
        let _ = read_reply(&mut stream)?;
        let user = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, request.username.as_bytes());
        let pass = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, request.password.as_bytes());
        write_line(&mut stream, &user)?;
        let _ = read_reply(&mut stream)?;
        write_line(&mut stream, &pass)?;
        let auth = read_reply(&mut stream)?;
        if !auth.starts_with("2") {
            return Err(AppError::msg(format!("SMTP 认证失败: {auth}")));
        }
    }
    write_line(&mut stream, &format!("MAIL FROM:<{}>", request.from))?;
    expect_2xx(&mut stream)?;
    write_line(&mut stream, &format!("RCPT TO:<{}>", request.to))?;
    expect_2xx(&mut stream)?;
    write_line(&mut stream, "DATA")?;
    expect_2xx(&mut stream)?;
    let dotted = message.replace("\r\n.", "\r\n..");
    stream
        .write_all(format!("{dotted}\r\n.\r\n").as_bytes())
        .map_err(|err| AppError::msg(err.to_string()))?;
    expect_2xx(&mut stream)?;
    write_line(&mut stream, "QUIT")?;
    Ok(())
}

fn expect_2xx<S: Read>(stream: &mut S) -> AppResult<()> {
    let reply = read_reply(stream)?;
    if reply.starts_with('2') || reply.starts_with('3') {
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
    let mut buffer = [0u8; 2048];
    let count = stream.read(&mut buffer).map_err(|err| AppError::msg(err.to_string()))?;
    Ok(String::from_utf8_lossy(&buffer[..count]).trim().to_string())
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
}
