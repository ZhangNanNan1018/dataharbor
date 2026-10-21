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
    load_cipher_with(config_dir, force_file, keyring_get, keyring_set)
}

#[derive(Debug)]
enum KeyringRead {
    Found([u8; 32]),
    Missing,
    Unavailable(String),
}

fn load_cipher_with(
    config_dir: &Path,
    force_file: bool,
    get: impl FnOnce() -> KeyringRead,
    set: impl FnOnce(&[u8; 32]) -> bool,
) -> AppResult<(CredentialCipher, KeySource)> {
    // Serialize first launch across the desktop and scheduled CLI processes.
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(config_dir.join("master-key.lock"))
        .map_err(|err| AppError::msg(format!("无法锁定主密钥：{err}")))?;
    lock.lock()
        .map_err(|err| AppError::msg(format!("无法锁定主密钥：{err}")))?;
    let path = config_dir.join("master.key");
    let file_key = if path.exists() {
        let bytes =
            fs::read(&path).map_err(|err| AppError::msg(format!("读取主密钥失败: {err}")))?;
        let key: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| AppError::msg("主密钥文件长度不正确"))?;
        Some(key)
    } else {
        None
    };
    if force_file {
        if let Some(key) = file_key {
            return Ok((CredentialCipher::from_key(key), KeySource::File));
        }
    }

    let read = if force_file {
        KeyringRead::Missing
    } else {
        get()
    };
    if let (Some(local), KeyringRead::Found(remote)) = (file_key, &read) {
        if local == *remote {
            return Ok((CredentialCipher::from_key(local), KeySource::File));
        }
        // Older versions could leave an unrelated fallback file after a keyring timeout.
        // Existing authenticated ciphertext is the only evidence for choosing between keys.
        let secrets = read_saved_secrets(config_dir)?;
        return match (
            decrypts_all(&local, &secrets),
            decrypts_all(remote, &secrets),
        ) {
            (true, false) => Ok((CredentialCipher::from_key(local), KeySource::File)),
            (false, true) => Ok((CredentialCipher::from_key(*remote), KeySource::Keyring)),
            _ => Err(AppError::msg("系统钥匙串与 master.key 的主密钥冲突，现有密文无法唯一确定正确密钥。已停止启动；两份密钥及原配置均未修改，请恢复匹配的原始凭据")),
        };
    }
    let secrets = read_saved_secrets(config_dir)?;
    if let Some(key) = file_key {
        if !secrets.is_empty() && !decrypts_all(&key, &secrets) {
            return Err(AppError::msg("master.key 无法解密现有凭据，系统钥匙串也未提供可用密钥。已保留全部文件，请解锁钥匙串或恢复匹配的主密钥后重试"));
        }
        return Ok((CredentialCipher::from_key(key), KeySource::File));
    }
    if let KeyringRead::Found(key) = read {
        if !secrets.is_empty() && !decrypts_all(&key, &secrets) {
            return Err(AppError::msg(
                "系统钥匙串主密钥无法解密现有凭据，已保留配置并停止启动，请恢复匹配的主密钥",
            ));
        }
        return Ok((CredentialCipher::from_key(key), KeySource::Keyring));
    }
    if !secrets.is_empty() {
        let reason = match read {
            KeyringRead::Unavailable(reason) => reason,
            _ => "主密钥不存在".into(),
        };
        return Err(AppError::msg(format!("{reason}；检测到已有加密凭据，已停止创建主密钥。请解锁系统钥匙串后重试，或恢复原 master.key；原配置未修改")));
    }
    let key = random_key();
    // A failed read is never permission to overwrite an existing keyring entry.
    if !force_file && matches!(read, KeyringRead::Missing) && set(&key) {
        return Ok((CredentialCipher::from_key(key), KeySource::Keyring));
    }
    write_key_file(&path, &key)?;
    Ok((CredentialCipher::from_key(key), KeySource::File))
}

fn decrypts_all(key: &[u8; 32], secrets: &[String]) -> bool {
    let cipher = CredentialCipher::from_key(*key);
    !secrets.is_empty() && secrets.iter().all(|secret| cipher.decrypt(secret).is_ok())
}

fn read_saved_secrets(config_dir: &Path) -> AppResult<Vec<String>> {
    fn collect_secrets(value: &serde_json::Value, named_secret: bool, secrets: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (name, value) in fields {
                    collect_secrets(
                        value,
                        matches!(
                            name.as_str(),
                            "passwordEnc"
                                | "sshPasswordEnc"
                                | "keyEnc"
                                | "password_enc"
                                | "ssh_password_enc"
                                | "key_enc"
                        ),
                        secrets,
                    );
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    collect_secrets(item, false, secrets);
                }
            }
            serde_json::Value::String(text)
                if !text.is_empty() && named_secret =>
            {
                secrets.push(text.clone());
            }
            _ => {}
        }
    }
    let mut secrets = Vec::new();
    for name in ["connections.json", "ai.json", "mail.json"] {
        let text = match fs::read_to_string(config_dir.join(name)) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(AppError::msg(format!(
                    "无法检查 {name}，已停止创建主密钥：{err}"
                )))
            }
        };
        if text.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str(&text).map_err(|err| {
            AppError::msg(format!(
                "{name} 无法解析，已保留配置并停止创建主密钥：{err}"
            ))
        })?;
        collect_secrets(&value, false, &mut secrets);
    }
    Ok(secrets)
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

fn keyring_get() -> KeyringRead {
    let service = SERVICE.to_string();
    with_keyring_timeout(move || {
        let entry = match keyring::Entry::new(&service, ACCOUNT) {
            Ok(entry) => entry,
            Err(_) => return KeyringRead::Unavailable("系统钥匙串不可用".into()),
        };
        match entry.get_password() {
            Ok(text) => match decode_key(&text) {
                Ok(key) => KeyringRead::Found(key),
                Err(err) => KeyringRead::Unavailable(err.to_string()),
            },
            Err(keyring::Error::NoEntry) => KeyringRead::Missing,
            Err(_) => KeyringRead::Unavailable("系统钥匙串暂时无法读取".into()),
        }
    })
    .unwrap_or_else(|| KeyringRead::Unavailable("系统钥匙串读取超时".into()))
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
    options.write(true).create_new(true);
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

    #[test]
    fn unavailable_or_missing_keyring_never_replaces_existing_secrets() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-key-recovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let stored = CredentialCipher::from_key([7; 32])
            .encrypt("saved password")
            .unwrap();
        let original = serde_json::json!({"profiles": [{"passwordEnc": stored}]}).to_string();
        fs::write(dir.join("connections.json"), &original).unwrap();
        for result in [
            KeyringRead::Missing,
            KeyringRead::Unavailable("timeout".into()),
        ] {
            let result = load_cipher_with(
                &dir,
                false,
                || result,
                |_| panic!("must not overwrite keyring"),
            );
            assert!(result.is_err());
            assert!(!dir.join("master.key").exists());
            assert_eq!(
                fs::read_to_string(dir.join("connections.json")).unwrap(),
                original
            );
        }
        let (cipher, _) =
            load_cipher_with(&dir, false, || KeyringRead::Found([7; 32]), |_| false).unwrap();
        assert_eq!(cipher.decrypt(&stored).unwrap(), "saved password");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fresh_unavailable_keyring_uses_file_without_writing_keyring() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-key-fallback-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let (cipher, source) = load_cipher_with(
            &dir,
            false,
            || KeyringRead::Unavailable("locked".into()),
            |_| panic!("unreadable is not missing"),
        )
        .unwrap();
        assert_eq!(source, KeySource::File);
        let stored = cipher.encrypt("fixture").unwrap();
        let (reopened, source) = load_cipher_with(
            &dir,
            false,
            || KeyringRead::Unavailable("still locked".into()),
            |_| false,
        )
        .unwrap();
        assert_eq!(source, KeySource::File);
        assert_eq!(reopened.decrypt(&stored).unwrap(), "fixture");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn conflicting_sources_select_only_the_key_that_decrypts_existing_secrets() {
        for file_matches in [false, true] {
            let dir = std::env::temp_dir()
                .join(format!("dataharbor-key-selection-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            let saved_key = [7; 32];
            let stale_key = [8; 32];
            let file_key = if file_matches { saved_key } else { stale_key };
            let ring_key = if file_matches { stale_key } else { saved_key };
            write_key_file(&dir.join("master.key"), &file_key).unwrap();
            let stored = CredentialCipher::from_key(saved_key)
                .encrypt("saved fixture")
                .unwrap();
            let config = serde_json::json!({"profiles": [{"passwordEnc": stored}]}).to_string();
            fs::write(dir.join("connections.json"), &config).unwrap();
            fs::write(
                dir.join("ai.json"),
                serde_json::json!({"keyEnc": stored}).to_string(),
            )
            .unwrap();
            let (cipher, source) = load_cipher_with(
                &dir,
                false,
                || KeyringRead::Found(ring_key),
                |_| panic!("must not overwrite either key"),
            )
            .unwrap();
            assert_eq!(
                source,
                if file_matches {
                    KeySource::File
                } else {
                    KeySource::Keyring
                }
            );
            assert_eq!(cipher.decrypt(&stored).unwrap(), "saved fixture");
            assert_eq!(fs::read(dir.join("master.key")).unwrap(), file_key);
            assert_eq!(
                fs::read_to_string(dir.join("connections.json")).unwrap(),
                config
            );
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn ambiguous_or_mixed_key_sources_fail_without_changing_files() {
        for mixed in [false, true] {
            let dir = std::env::temp_dir()
                .join(format!("dataharbor-key-conflict-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).unwrap();
            write_key_file(&dir.join("master.key"), &[7; 32]).unwrap();
            let config = if mixed {
                serde_json::json!({"profiles": [
                    {"passwordEnc": CredentialCipher::from_key([7; 32]).encrypt("one").unwrap()},
                    {"passwordEnc": CredentialCipher::from_key([8; 32]).encrypt("two").unwrap()},
                ]})
                .to_string()
            } else {
                "{\"profiles\":[]}".into()
            };
            fs::write(dir.join("connections.json"), &config).unwrap();
            let error = load_cipher_with(
                &dir,
                false,
                || KeyringRead::Found([8; 32]),
                |_| panic!("must not replace a key"),
            )
            .err()
            .unwrap();
            assert!(error.to_string().contains("主密钥冲突"));
            assert_eq!(fs::read(dir.join("master.key")).unwrap(), [7; 32]);
            assert_eq!(
                fs::read_to_string(dir.join("connections.json")).unwrap(),
                config
            );
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn matching_key_sources_are_accepted_without_regeneration() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-key-matching-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        write_key_file(&dir.join("master.key"), &[7; 32]).unwrap();
        let (cipher, source) = load_cipher_with(
            &dir,
            false,
            || KeyringRead::Found([7; 32]),
            |_| panic!("must not regenerate"),
        )
        .unwrap();
        assert_eq!(source, KeySource::File);
        let stored = CredentialCipher::from_key([7; 32])
            .encrypt("fixture")
            .unwrap();
        assert_eq!(cipher.decrypt(&stored).unwrap(), "fixture");
        fs::remove_dir_all(dir).unwrap();
    }
}
