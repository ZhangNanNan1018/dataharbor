use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::rngs::OsRng;
use rand::RngCore;

use crate::error::{AppError, AppResult};

const PREFIX: &str = "xb1:";
const SERVICE: &str = "dataharbor";
const ACCOUNT: &str = "master-key";

#[derive(Clone)]
pub struct CredentialCipher {
    key: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Keyring,
    File,
}

impl KeySource {
    pub fn label(self) -> &'static str {
        match self {
            KeySource::Keyring => "系统钥匙串",
            KeySource::File => "本地密钥文件",
        }
    }
}

impl CredentialCipher {
    pub fn from_key(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub fn encrypt(&self, plaintext: &str) -> AppResult<String> {
        if plaintext.is_empty() {
            return Ok(String::new());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|err| AppError::msg(format!("加密初始化失败: {err}")))?;
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| AppError::msg("加密失败"))?;
        let mut packed = Vec::with_capacity(12 + ciphertext.len());
        packed.extend_from_slice(&nonce_bytes);
        packed.extend_from_slice(&ciphertext);
        Ok(format!("{PREFIX}{}", STANDARD.encode(packed)))
    }

    pub fn decrypt(&self, stored: &str) -> AppResult<String> {
        if stored.is_empty() {
            return Ok(String::new());
        }
        let payload = stored.strip_prefix(PREFIX).unwrap_or(stored);
        let packed = STANDARD
            .decode(payload)
            .map_err(|_| AppError::msg("凭据损坏"))?;
        if packed.len() < 13 {
            return Err(AppError::msg("凭据损坏"));
        }
        let (nonce_bytes, ciphertext) = packed.split_at(12);
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|err| AppError::msg(format!("解密初始化失败: {err}")))?;
        let nonce = Nonce::from_slice(nonce_bytes);
        let plain = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| AppError::msg("无法解密凭据，主密钥不匹配"))?;
        String::from_utf8(plain).map_err(|_| AppError::msg("凭据损坏"))
    }
}

pub fn load_or_create_cipher(
    config_dir: &Path,
    force_file: bool,
) -> AppResult<(CredentialCipher, KeySource)> {
    if !force_file {
        if let Some(key) = keyring_get() {
            return Ok((CredentialCipher::from_key(key), KeySource::Keyring));
        }
    }

    let path = config_dir.join("master.key");
    if path.exists() {
        let bytes = fs::read(&path).map_err(|err| AppError::msg(format!("读取主密钥失败: {err}")))?;
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| AppError::msg("主密钥文件长度不正确"))?;
        return Ok((CredentialCipher::from_key(key), KeySource::File));
    }

    let key = random_key();
    if !force_file && keyring_set(&key) {
        return Ok((CredentialCipher::from_key(key), KeySource::Keyring));
    }
    write_key_file(&path, &key)?;
    Ok((CredentialCipher::from_key(key), KeySource::File))
}

fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

pub fn migrate_keyring(from_service: &str, to_service: &str) -> bool {
    if from_service == to_service {
        return true;
    }
    let from_service = from_service.to_string();
    let to_service = to_service.to_string();
    with_keyring_timeout(move || copy_keyring_secret(&from_service, &to_service)).unwrap_or(false)
}

fn copy_keyring_secret(from_service: &str, to_service: &str) -> bool {
    let Ok(old_entry) = keyring::Entry::new(from_service, ACCOUNT) else {
        return false;
    };
    let Ok(secret) = old_entry.get_password() else {
        return false;
    };
    if decode_key(&secret).is_err() {
        return false;
    }
    let Ok(new_entry) = keyring::Entry::new(to_service, ACCOUNT) else {
        return false;
    };
    match new_entry.get_password() {
        Ok(_) => true,
        Err(keyring::Error::NoEntry) => {
            if new_entry.set_password(&secret).is_ok() {
                let _ = old_entry.delete_credential();
                true
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

fn keyring_get() -> Option<[u8; 32]> {
    let service = SERVICE.to_string();
    with_keyring_timeout(move || {
        let entry = keyring::Entry::new(&service, ACCOUNT).ok()?;
        match entry.get_password() {
            Ok(text) => decode_key(&text).ok(),
            Err(_) => None,
        }
    })
    .flatten()
}

fn keyring_set(key: &[u8; 32]) -> bool {
    let key = *key;
    let service = SERVICE.to_string();
    with_keyring_timeout(move || {
        let Ok(entry) = keyring::Entry::new(&service, ACCOUNT) else {
            return false;
        };
        entry.set_password(&STANDARD.encode(key)).is_ok()
    })
    .unwrap_or(false)
}

fn with_keyring_timeout<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(Duration::from_secs(2)).ok()
}

fn decode_key(text: &str) -> AppResult<[u8; 32]> {
    let bytes = STANDARD
        .decode(text.trim())
        .map_err(|_| AppError::msg("钥匙串中的主密钥无法解析"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| AppError::msg("钥匙串中的主密钥长度不正确"))
}

fn write_key_file(path: &Path, key: &[u8; 32]) -> AppResult<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| AppError::msg(format!("写入主密钥失败: {err}")))?;
    file.write_all(key)
        .map_err(|err| AppError::msg(format!("写入主密钥失败: {err}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_unicode_secret() {
        let cipher = CredentialCipher::from_key([7u8; 32]);
        let secret = "p@ss 密码 '\"\\";
        let stored = cipher.encrypt(secret).unwrap();
        assert!(stored.starts_with(PREFIX));
        assert!(!stored.contains(secret));
        assert_eq!(cipher.decrypt(&stored).unwrap(), secret);
        assert_eq!(cipher.decrypt("").unwrap(), "");
    }

    #[test]
    fn rejects_wrong_key() {
        let cipher = CredentialCipher::from_key([1u8; 32]);
        let other = CredentialCipher::from_key([2u8; 32]);
        let stored = cipher.encrypt("secret").unwrap();
        assert!(other.decrypt(&stored).is_err());
    }
}
