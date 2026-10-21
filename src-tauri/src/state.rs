use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::crypto::{self, CredentialCipher, KeySource};
use crate::drivers::{self, DbSession};
use crate::error::{AppError, AppResult};
use crate::models::{
    AppInfo, ConnectResult, ConnectionPublic, Engine, SaveConnectionRequest, SaveConnectionResult,
    SessionConfig, StoredProfile, TestResult,
};
use crate::store;
use crate::tunnel::TunnelGuard;

pub struct SessionSlot {
    pub inner: Mutex<Box<dyn DbSession>>,
    pub tunnel: Vec<TunnelGuard>,
    pub live: SessionConfig,
}

/// A task owns its database transaction and pins any SSH/HTTP tunnel until it ends.
pub(crate) struct IsolatedSession {
    inner: Box<dyn DbSession>,
    _owner: Arc<SessionSlot>,
}
impl std::ops::Deref for IsolatedSession {
    type Target = dyn DbSession;
    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}
impl std::ops::DerefMut for IsolatedSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.inner
    }
}

impl SessionSlot {
    pub fn tunnel_count(&self) -> usize {
        self.tunnel.len()
    }
}

pub struct AppState {
    pub(crate) operations: crate::operations::OperationManager,
    config_dir: PathBuf,
    cipher: CredentialCipher,
    key_source: KeySource,
    profiles: Mutex<Vec<StoredProfile>>,
    session_lifecycle: Mutex<()>,
    sessions: Mutex<HashMap<String, Arc<SessionSlot>>>,
    debuggers: Mutex<HashMap<String, crate::pldbg::DebugSlot>>,
}

impl AppState {
    pub fn initialize() -> AppResult<Self> {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        crate::migrate::prepare_local_data(&base)?;
        Self::open_in(base.join(crate::migrate::CONFIG_DIR_NAME), false)
    }

    pub fn open_in(dir: PathBuf, force_file: bool) -> AppResult<Self> {
        fs::create_dir_all(&dir)
            .map_err(|err| AppError::msg(format!("无法创建配置目录: {err}")))?;
        let (cipher, key_source) = crypto::load_or_create_cipher(&dir, force_file)?;
        let profiles = store::load_profiles(&dir)?;
        Ok(Self {
            operations: crate::operations::OperationManager::default(),
            config_dir: dir,
            cipher,
            key_source,
            profiles: Mutex::new(profiles),
            session_lifecycle: Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
            debuggers: Mutex::new(HashMap::new()),
        })
    }

    pub(crate) fn debuggers(&self) -> &Mutex<HashMap<String, crate::pldbg::DebugSlot>> {
        &self.debuggers
    }

    pub(crate) fn live_config(&self, id: &str) -> AppResult<SessionConfig> {
        let sessions = lock(&self.sessions);
        sessions
            .get(id)
            .map(|slot| slot.live.clone())
            .ok_or_else(|| AppError::msg("连接未打开，请先在左侧连接"))
    }

    pub fn app_info(&self) -> AppInfo {
        AppInfo {
            name: "DataHarbor".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            encryption: self.key_source.label().to_string(),
            config_dir: self.config_dir.display().to_string(),
            independent: true,
        }
    }

    pub fn list_connections(&self) -> Vec<ConnectionPublic> {
        let profiles = lock(&self.profiles);
        let sessions = lock(&self.sessions);
        profiles
            .iter()
            .map(|profile| self.to_public(profile, sessions.contains_key(&profile.id)))
            .collect()
    }

    pub fn save_connection(
        &self,
        request: SaveConnectionRequest,
    ) -> AppResult<SaveConnectionResult> {
        let name = request.name.trim();
        if name.is_empty() {
            return Err(AppError::msg("请填写连接名称"));
        }
        if name.chars().count() > 80 {
            return Err(AppError::msg("连接名称过长"));
        }
        let engine = request.engine;
        let mut port = request.port;
        if port == 0 {
            port = engine.default_port();
        }
        let host = request.host.trim().to_string();
        let file_path = request.file_path.trim().to_string();
        let database = request.database.trim().to_string();
        let username = request.username.trim().to_string();
        if engine == Engine::Sqlite && file_path.is_empty() {
            return Err(AppError::msg("请选择 SQLite 数据库文件"));
        }
        if engine != Engine::Sqlite && host.is_empty() {
            return Err(AppError::msg("请填写主机地址"));
        }
        let color = normalize_color(&request.color);
        let group = request.group.trim().chars().take(160).collect::<String>();
        let ssh_port = if request.ssh_port == 0 {
            22
        } else {
            request.ssh_port
        };
        let now = now_string();

        let _lifecycle = lock(&self.session_lifecycle);
        let _file_lock = self.profile_store_lock()?;
        let mut cached_profiles = lock(&self.profiles);
        let mut profiles = store::load_profiles(&self.config_dir)?;
        let (public, needs_reconnect) =
            if let Some(id) = request.id.as_ref().filter(|id| !id.is_empty()) {
                let existing = profiles
                    .iter_mut()
                    .find(|profile| &profile.id == id)
                    .ok_or_else(|| AppError::msg("连接不存在"))?;
                let mut password_changed = false;
                if request.clear_password {
                    existing.password_enc.clear();
                    password_changed = true;
                } else if let Some(password) = request.password.as_ref().map(|item| item.as_str()) {
                    if !password.is_empty() {
                        existing.password_enc = self.cipher.encrypt(password)?;
                        password_changed = true;
                    }
                }
                let params_changed = password_changed
                    || existing.engine != engine
                    || existing.host != host
                    || existing.port != port
                    || existing.username != username
                    || existing.database != database
                    || existing.file_path != file_path
                    || existing.use_ssl != request.use_ssl
                    || existing.ssl_insecure != request.ssl_insecure
                    || existing.ssh_enabled != request.ssh_enabled
                    || existing.ssh_host != request.ssh_host.trim()
                    || existing.ssh_port != ssh_port
                    || existing.ssh_username != request.ssh_username.trim()
                    || existing.ssh_key_path != request.ssh_key_path.trim()
                    || request.clear_ssh_password
                    || request
                        .ssh_password
                        .as_ref()
                        .is_some_and(|value| !value.is_empty())
                    || existing.http_tunnel != request.http_tunnel
                    || existing.http_proxy_host != request.http_proxy_host.trim()
                    || existing.http_proxy_port
                        != if request.http_proxy_port == 0 {
                            8080
                        } else {
                            request.http_proxy_port
                        }
                    || existing.server_mode != request.server_mode.trim()
                    || existing.auth_method != normalize_auth(&request.auth_method)
                    || existing.warehouse != request.warehouse.trim()
                    || existing.schema_name != request.schema_name.trim()
                    || existing.address_map != request.address_map.trim();
                existing.name = name.to_string();
                existing.group = group;
                existing.color = color;
                existing.engine = engine;
                existing.host = host;
                existing.port = port;
                existing.username = username;
                existing.database = database;
                existing.file_path = file_path;
                existing.use_ssl = request.use_ssl;
                existing.ssl_insecure = request.ssl_insecure;
                if request.clear_ssh_password {
                    existing.ssh_password_enc.clear();
                } else if let Some(password) = request
                    .ssh_password
                    .as_ref()
                    .filter(|item| !item.is_empty())
                {
                    existing.ssh_password_enc = self.cipher.encrypt(password)?;
                }
                existing.ssh_enabled = request.ssh_enabled;
                existing.ssh_host = request.ssh_host.trim().to_string();
                existing.ssh_port = ssh_port;
                existing.ssh_username = request.ssh_username.trim().to_string();
                existing.ssh_key_path = request.ssh_key_path.trim().to_string();
                existing.http_tunnel = request.http_tunnel;
                existing.http_proxy_host = request.http_proxy_host.trim().to_string();
                existing.http_proxy_port = if request.http_proxy_port == 0 {
                    8080
                } else {
                    request.http_proxy_port
                };
                existing.server_mode = request.server_mode.trim().to_string();
                existing.auth_method = normalize_auth(&request.auth_method);
                existing.warehouse = request.warehouse.trim().to_string();
                existing.schema_name = request.schema_name.trim().to_string();
                existing.address_map = request.address_map.trim().to_string();
                existing.updated_at = now;
                let public = self.to_public(existing, false);
                (public, params_changed)
            } else {
                let mut password_enc = String::new();
                if let Some(password) = request.password.as_ref().filter(|item| !item.is_empty()) {
                    password_enc = self.cipher.encrypt(password)?;
                }
                let mut ssh_password_enc = String::new();
                if let Some(password) = request
                    .ssh_password
                    .as_ref()
                    .filter(|item| !item.is_empty())
                {
                    ssh_password_enc = self.cipher.encrypt(password)?;
                }
                let profile = StoredProfile {
                    id: Uuid::new_v4().to_string(),
                    name: name.to_string(),
                    group,
                    color,
                    engine,
                    host,
                    port,
                    username,
                    password_enc,
                    database,
                    file_path,
                    use_ssl: request.use_ssl,
                    ssl_insecure: request.ssl_insecure,
                    ssh_enabled: request.ssh_enabled,
                    ssh_host: request.ssh_host.trim().to_string(),
                    ssh_port,
                    ssh_username: request.ssh_username.trim().to_string(),
                    ssh_password_enc,
                    ssh_key_path: request.ssh_key_path.trim().to_string(),
                    http_tunnel: request.http_tunnel,
                    http_proxy_host: request.http_proxy_host.trim().to_string(),
                    http_proxy_port: if request.http_proxy_port == 0 {
                        8080
                    } else {
                        request.http_proxy_port
                    },
                    server_mode: request.server_mode.trim().to_string(),
                    auth_method: normalize_auth(&request.auth_method),
                    warehouse: request.warehouse.trim().to_string(),
                    schema_name: request.schema_name.trim().to_string(),
                    address_map: request.address_map.trim().to_string(),
                    created_at: now.clone(),
                    updated_at: now,
                };
                let public = self.to_public(&profile, false);
                profiles.push(profile);
                (public, false)
            };
        store::save_profiles(&self.config_dir, &profiles)?;
        *cached_profiles = profiles;
        drop(cached_profiles);
        if needs_reconnect {
            self.drop_session(&public.id);
        }
        let connected = lock(&self.sessions).contains_key(&public.id);
        let mut public = public;
        public.connected = connected;
        Ok(SaveConnectionResult {
            connection: public,
            needs_reconnect,
        })
    }

    pub fn delete_connection(&self, id: &str) -> AppResult<()> {
        let _lifecycle = lock(&self.session_lifecycle);
        let _file_lock = self.profile_store_lock()?;
        let mut cached_profiles = lock(&self.profiles);
        let mut profiles = store::load_profiles(&self.config_dir)?;
        let before = profiles.len();
        profiles.retain(|profile| profile.id != id);
        if profiles.len() == before {
            return Err(AppError::msg("连接不存在"));
        }
        store::save_profiles(&self.config_dir, &profiles)?;
        *cached_profiles = profiles;
        drop(cached_profiles);
        self.drop_session(id);
        Ok(())
    }

    pub fn test_connection(&self, request: SaveConnectionRequest) -> AppResult<TestResult> {
        let mut config = self.config_from_request(&request)?;
        let profile = profile_from_request(&request);
        let _tunnels = open_with_tunnels(&profile, &mut config)?;
        let mut session = drivers::open_session(&config)?;
        match session.version() {
            Ok(version) => Ok(TestResult {
                ok: true,
                message: format!("连接成功 · {version}"),
                version: Some(version),
            }),
            Err(err) => Ok(TestResult {
                ok: false,
                message: err.to_string(),
                version: None,
            }),
        }
    }

    pub fn connect(&self, id: &str) -> AppResult<ConnectResult> {
        let _lifecycle = lock(&self.session_lifecycle);
        let profile = self.profile_by_id(id)?;
        if lock(&self.sessions).contains_key(id) {
            match self.with_session(id, |session| session.version()) {
                Ok(version) => {
                    return Ok(ConnectResult {
                        id: id.to_string(),
                        engine: profile.engine,
                        version,
                        database: display_database(&profile),
                    });
                }
                Err(_) => self.drop_session(id),
            }
        }
        let mut config = self.session_config(&profile)?;
        let tunnels = open_with_tunnels(&profile, &mut config)?;
        let mut session = drivers::open_session(&config)?;
        let version = session.version()?;
        let slot = Arc::new(SessionSlot {
            inner: Mutex::new(session),
            tunnel: tunnels,
            live: config,
        });
        let tunnel_count = slot.tunnel_count();
        lock(&self.sessions).insert(id.to_string(), slot);
        let version = if tunnel_count > 1 {
            format!("{version} · {tunnel_count} 条 SSH/HTTP 隧道")
        } else {
            version
        };
        Ok(ConnectResult {
            id: id.to_string(),
            engine: profile.engine,
            version,
            database: display_database(&profile),
        })
    }

    pub fn disconnect(&self, id: &str) {
        let _lifecycle = lock(&self.session_lifecycle);
        self.drop_session(id);
    }

    pub fn create_sample_database(&self) -> AppResult<ConnectionPublic> {
        let samples = self.config_dir.join("samples");
        fs::create_dir_all(&samples)
            .map_err(|err| AppError::msg(format!("无法创建示例目录: {err}")))?;
        let path = samples.join("bookstore.sqlite");
        seed_bookstore(&path)?;
        let file_path = path.display().to_string();
        let existing = lock(&self.profiles)
            .iter()
            .find(|profile| profile.engine == Engine::Sqlite && profile.file_path == file_path)
            .map(|profile| profile.id.clone());
        if let Some(id) = existing {
            return self
                .list_connections()
                .into_iter()
                .find(|profile| profile.id == id)
                .ok_or_else(|| AppError::msg("示例连接丢失"));
        }
        let saved = self.save_connection(SaveConnectionRequest {
            id: None,
            name: "示例书店".to_string(),
            group: "示例".to_string(),
            color: "#3cba8e".to_string(),
            engine: Engine::Sqlite,
            host: String::new(),
            port: 0,
            username: String::new(),
            password: None,
            clear_password: false,
            database: String::new(),
            file_path,
            use_ssl: false,
            ssl_insecure: false,
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: 22,
            ssh_username: String::new(),
            ..SaveConnectionRequest::default()
        })?;
        Ok(saved.connection)
    }

    fn profile_store_lock(&self) -> AppResult<fs::File> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.config_dir.join("connections.json.lock"))
            .map_err(|err| AppError::msg(format!("无法锁定连接配置：{err}")))?;
        file.lock()
            .map_err(|err| AppError::msg(format!("无法锁定连接配置：{err}")))?;
        Ok(file)
    }

    pub(crate) fn config_dir(&self) -> &std::path::Path {
        &self.config_dir
    }

    pub(crate) fn encrypt_secret(&self, text: &str) -> AppResult<String> {
        self.cipher.encrypt(text)
    }

    pub(crate) fn decrypt_secret(&self, text: &str) -> AppResult<String> {
        self.cipher.decrypt(text)
    }

    pub(crate) fn profiles_snapshot(&self) -> Vec<StoredProfile> {
        lock(&self.profiles).clone()
    }

    pub(crate) fn import_profiles(&self, extra: Vec<StoredProfile>) -> AppResult<(usize, usize)> {
        let _lifecycle = lock(&self.session_lifecycle);
        let _file_lock = self.profile_store_lock()?;
        let mut cached_profiles = lock(&self.profiles);
        let mut profiles = store::load_profiles(&self.config_dir)?;
        let mut added = 0;
        let mut needs_credentials = 0;
        for mut item in extra {
            if item.id.is_empty() || profiles.iter().any(|profile| profile.id == item.id) {
                continue;
            }
            let mut cleared = false;
            for secret in [&mut item.password_enc, &mut item.ssh_password_enc] {
                if !secret.is_empty() && self.cipher.decrypt(secret).is_err() {
                    secret.clear();
                    cleared = true;
                }
            }
            needs_credentials += usize::from(cleared);
            profiles.push(item);
            added += 1;
        }
        store::save_profiles(&self.config_dir, &profiles)?;
        *cached_profiles = profiles;
        drop(cached_profiles);
        Ok((added, needs_credentials))
    }

    pub(crate) fn session_config_for(&self, id: &str) -> AppResult<SessionConfig> {
        let profile = self.profile_by_id(id)?;
        self.session_config(&profile)
    }

    pub(crate) fn with_session<T>(
        &self,
        id: &str,
        f: impl FnOnce(&mut dyn DbSession) -> AppResult<T>,
    ) -> AppResult<T> {
        let slot = {
            let sessions = lock(&self.sessions);
            sessions
                .get(id)
                .cloned()
                .ok_or_else(|| AppError::msg("连接未打开，请先在左侧连接"))?
        };
        let mut guard = lock(&slot.inner);
        f(&mut **guard)
    }

    pub(crate) fn with_session_operation<T>(
        &self,
        id: &str,
        database: &str,
        operation: &crate::operations::OperationContext,
        f: impl FnOnce(&mut dyn DbSession) -> AppResult<T>,
    ) -> AppResult<T> {
        self.with_session_wait(id, operation, |session| {
            let interrupt = operation.bind_interrupt(session.cancel_handle(database)?)?;
            operation.checkpoint()?;
            let result = f(session);
            drop(interrupt);
            if result.is_err() && operation.cancel_requested() {
                return Err(operation.cancel_error());
            }
            result
        })
    }

    pub(crate) fn with_session_wait<T>(
        &self,
        id: &str,
        operation: &crate::operations::OperationContext,
        f: impl FnOnce(&mut dyn DbSession) -> AppResult<T>,
    ) -> AppResult<T> {
        operation.checkpoint()?;
        let slot = lock(&self.sessions)
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::msg("连接未打开，请先在左侧连接"))?;
        let mut session = loop {
            operation.checkpoint()?;
            match slot.inner.try_lock() {
                Ok(session) => break session,
                Err(TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(15))
                }
            }
        };
        operation.checkpoint()?;
        f(&mut **session)
    }

    pub(crate) fn open_isolated_session(&self, id: &str) -> AppResult<IsolatedSession> {
        let existing = { lock(&self.sessions).get(id).cloned() };
        let slot = if let Some(slot) = existing {
            slot
        } else {
            self.connect(id)?;
            lock(&self.sessions)
                .get(id)
                .cloned()
                .ok_or_else(|| AppError::msg("连接已关闭"))?
        };
        let inner = drivers::open_session(&slot.live)?;
        Ok(IsolatedSession {
            inner,
            _owner: slot,
        })
    }

    fn profile_by_id(&self, id: &str) -> AppResult<StoredProfile> {
        lock(&self.profiles)
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
            .ok_or_else(|| AppError::msg("连接不存在"))
    }

    fn session_config(&self, profile: &StoredProfile) -> AppResult<SessionConfig> {
        let password = if profile.password_enc.is_empty() {
            String::new()
        } else {
            self.cipher.decrypt(&profile.password_enc)?
        };
        let ssh_password = if profile.ssh_password_enc.is_empty() {
            String::new()
        } else {
            self.cipher.decrypt(&profile.ssh_password_enc)?
        };
        Ok(SessionConfig {
            engine: profile.engine,
            host: profile.host.clone(),
            port: profile.port,
            tls_server_name: None,
            username: profile.username.clone(),
            password,
            database: profile.database.clone(),
            file_path: profile.file_path.clone(),
            use_ssl: profile.use_ssl,
            ssl_insecure: profile.ssl_insecure,
            ssh_password,
            server_mode: profile.server_mode.clone(),
            auth_method: profile.auth_method.clone(),
            warehouse: profile.warehouse.clone(),
            schema_name: profile.schema_name.clone(),
            address_map: profile.address_map.clone(),
        })
    }

    fn config_from_request(&self, request: &SaveConnectionRequest) -> AppResult<SessionConfig> {
        let mut password = request.password.clone().unwrap_or_default();
        if password.is_empty() {
            if let Some(id) = request.id.as_ref().filter(|id| !id.is_empty()) {
                if let Ok(profile) = self.profile_by_id(id) {
                    if !profile.password_enc.is_empty() && !request.clear_password {
                        password = self.cipher.decrypt(&profile.password_enc)?;
                    }
                }
            }
        }
        let mut port = request.port;
        if port == 0 {
            port = request.engine.default_port();
        }
        let mut ssh_password = request.ssh_password.clone().unwrap_or_default();
        if ssh_password.is_empty() {
            if let Some(id) = request.id.as_ref().filter(|id| !id.is_empty()) {
                if let Ok(profile) = self.profile_by_id(id) {
                    if !profile.ssh_password_enc.is_empty() && !request.clear_ssh_password {
                        ssh_password = self.cipher.decrypt(&profile.ssh_password_enc)?;
                    }
                }
            }
        }
        Ok(SessionConfig {
            engine: request.engine,
            host: request.host.trim().to_string(),
            port,
            tls_server_name: None,
            username: request.username.clone(),
            password,
            database: request.database.trim().to_string(),
            file_path: request.file_path.trim().to_string(),
            use_ssl: request.use_ssl,
            ssl_insecure: request.ssl_insecure,
            ssh_password,
            server_mode: request.server_mode.trim().to_string(),
            auth_method: normalize_auth(&request.auth_method),
            warehouse: request.warehouse.trim().to_string(),
            schema_name: request.schema_name.trim().to_string(),
            address_map: request.address_map.trim().to_string(),
        })
    }

    fn to_public(&self, profile: &StoredProfile, connected: bool) -> ConnectionPublic {
        ConnectionPublic {
            id: profile.id.clone(),
            name: profile.name.clone(),
            group: profile.group.clone(),
            color: profile.color.clone(),
            engine: profile.engine,
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            has_password: !profile.password_enc.is_empty(),
            database: profile.database.clone(),
            file_path: profile.file_path.clone(),
            use_ssl: profile.use_ssl,
            ssl_insecure: profile.ssl_insecure,
            ssh_enabled: profile.ssh_enabled,
            ssh_host: profile.ssh_host.clone(),
            ssh_port: profile.ssh_port,
            ssh_username: profile.ssh_username.clone(),
            has_ssh_password: !profile.ssh_password_enc.is_empty(),
            ssh_key_path: profile.ssh_key_path.clone(),
            http_tunnel: profile.http_tunnel,
            http_proxy_host: profile.http_proxy_host.clone(),
            http_proxy_port: profile.http_proxy_port,
            server_mode: profile.server_mode.clone(),
            auth_method: profile.auth_method.clone(),
            warehouse: profile.warehouse.clone(),
            schema_name: profile.schema_name.clone(),
            address_map: profile.address_map.clone(),
            connected,
            placeholder: false,
        }
    }

    fn drop_session(&self, id: &str) {
        crate::pldbg::discard(self, id);
        lock(&self.sessions).remove(id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn normalize_color(color: &str) -> String {
    let color = color.trim();
    if color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|ch| ch.is_ascii_hexdigit())
    {
        color.to_ascii_lowercase()
    } else {
        "#6366f1".to_string()
    }
}

fn profile_from_request(request: &SaveConnectionRequest) -> StoredProfile {
    StoredProfile {
        id: request.id.clone().unwrap_or_default(),
        name: request.name.clone(),
        group: request.group.clone(),
        color: request.color.clone(),
        engine: request.engine,
        host: request.host.clone(),
        port: request.port,
        username: request.username.clone(),
        password_enc: String::new(),
        database: request.database.clone(),
        file_path: request.file_path.clone(),
        use_ssl: request.use_ssl,
        ssl_insecure: request.ssl_insecure,
        ssh_enabled: request.ssh_enabled,
        ssh_host: request.ssh_host.clone(),
        ssh_port: if request.ssh_port == 0 {
            22
        } else {
            request.ssh_port
        },
        ssh_username: request.ssh_username.clone(),
        ssh_password_enc: String::new(),
        ssh_key_path: request.ssh_key_path.clone(),
        http_tunnel: request.http_tunnel,
        http_proxy_host: request.http_proxy_host.clone(),
        http_proxy_port: if request.http_proxy_port == 0 {
            8080
        } else {
            request.http_proxy_port
        },
        server_mode: request.server_mode.clone(),
        auth_method: normalize_auth(&request.auth_method),
        warehouse: request.warehouse.clone(),
        schema_name: request.schema_name.clone(),
        address_map: request.address_map.clone(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

fn normalize_auth(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        "password".into()
    } else {
        value
    }
}

fn open_with_tunnels(
    profile: &StoredProfile,
    config: &mut SessionConfig,
) -> AppResult<Vec<TunnelGuard>> {
    validate_tunnel_security(profile, config)?;
    let cluster = profile.engine == Engine::Redis
        && profile.server_mode.eq_ignore_ascii_case("cluster")
        && (profile.ssh_enabled || profile.http_tunnel);
    if cluster {
        let nodes = config
            .host
            .split([',', ' '])
            .filter(|item| !item.trim().is_empty())
            .map(|item| split_host_port(item.trim(), config.port))
            .collect::<Vec<_>>();
        if nodes.is_empty() {
            return Err(AppError::msg("Redis Cluster 需要至少一个节点"));
        }
        let mut guards = Vec::new();
        let mut locals = Vec::new();
        let mut pairs = Vec::new();
        for (host, port) in nodes {
            let guard = crate::tunnel::start(profile, &config.ssh_password, &host, port)?
                .ok_or_else(|| {
                    AppError::msg(format!("没有为 Redis 节点 {host}:{port} 建立隧道"))
                })?;
            let local = format!("127.0.0.1:{}", guard.local_port());
            locals.push(local.clone());
            pairs.push(format!("{host}:{port}={local}"));
            guards.push(guard);
        }
        config.host = locals.join(",");
        config.port = 6379;
        config.address_map =
            crate::drivers::redis::merge_address_map(&config.address_map, &pairs.join(","));
        return Ok(guards);
    }
    let tunnel = crate::tunnel::start(profile, &config.ssh_password, &config.host, config.port)?;
    if let Some(tunnel) = tunnel {
        apply_tunnel_destination(config, tunnel.local_port());
        Ok(vec![tunnel])
    } else {
        Ok(Vec::new())
    }
}

fn apply_tunnel_destination(config: &mut SessionConfig, local_port: u16) {
    config.tls_server_name = Some(
        config
            .host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
    );
    config.host = "127.0.0.1".into();
    config.port = local_port;
}

fn validate_tunnel_security(profile: &StoredProfile, config: &SessionConfig) -> AppResult<()> {
    if !profile.ssh_enabled && !profile.http_tunnel {
        return Ok(());
    }
    if config.engine == Engine::Snowflake {
        return Err(AppError::msg(
            "当前 Snowflake HTTPS 驱动不支持 SSH / HTTP 隧道，请使用可直连的 HTTPS 端点。",
        ));
    }
    if config.host.contains("://") {
        return Err(AppError::msg(
            "隧道目标需要填写单独的主机名或 IP，不能直接使用连接 URI。",
        ));
    }
    if config.use_ssl {
        match config.engine {
            Engine::Mysql | Engine::Mariadb if !config.ssl_insecure => {
                return Err(AppError::msg("当前 MySQL / MariaDB 驱动不能分别设置隧道地址和 TLS 证书主机名，无法安全验证远端证书，已拒绝此组合。请使用直接 TLS 连接；程序不会自动关闭证书校验。"));
            }
            Engine::Mongodb if !config.ssl_insecure => {
                return Err(AppError::msg("当前 MongoDB 驱动不能为隧道单独指定 TLS 证书主机名，已拒绝此组合。请使用直接 TLS 连接；程序不会自动关闭证书校验。"));
            }
            Engine::Dameng => return Err(AppError::msg("当前达梦驱动不能分别设置隧道地址和 TLS 证书主机名，已拒绝 SSL 隧道连接；请使用直接 TLS 连接。")),
            Engine::Redis => return Err(AppError::msg("当前 Redis 构建未包含 TLS 支持，不能启用 SSL；程序不会退回明文连接。")),
            Engine::Oracle => return Err(AppError::msg("当前 Oracle SQL*Plus 连接仅实现 TCP，未实现 TCPS / 钱包配置，不能启用 SSL；程序不会退回明文连接。")),
            _ => {}
        }
    }
    Ok(())
}

fn split_host_port(item: &str, default_port: u16) -> (String, u16) {
    if let Ok(address) = item.parse::<std::net::SocketAddr>() {
        return (address.ip().to_string(), address.port());
    }
    if let Some((host, port)) = item.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') {
            if let Ok(port) = port.parse::<u16>() {
                return (host.to_string(), port);
            }
        }
    }
    let port = if default_port == 0 {
        6379
    } else {
        default_port
    };
    (
        item.trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        port,
    )
}

fn display_database(profile: &StoredProfile) -> String {
    match profile.engine {
        Engine::Sqlite => "main".to_string(),
        Engine::Postgres if profile.database.trim().is_empty() => "postgres".to_string(),
        _ => profile.database.clone(),
    }
}

fn seed_bookstore(path: &std::path::Path) -> AppResult<()> {
    let conn = rusqlite::Connection::open(path).map_err(|err| AppError::msg(err.to_string()))?;
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'authors'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    if exists > 0 {
        return Ok(());
    }
    conn.execute_batch(
        "
        CREATE TABLE authors (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            country TEXT
        );
        CREATE TABLE books (
            id INTEGER PRIMARY KEY,
            title TEXT NOT NULL,
            author_id INTEGER,
            price REAL,
            published_on TEXT
        );
        CREATE VIEW book_catalog AS
        SELECT b.id AS id,
               b.title AS title,
               a.name AS author,
               b.price AS price,
               b.published_on AS published_on
        FROM books b
        LEFT JOIN authors a ON a.id = b.author_id;
        INSERT INTO authors (id, name, country) VALUES
            (1, '余华', '中国'),
            (2, '老舍', '中国'),
            (3, '东野圭吾', '日本'),
            (4, '阿加莎·克里斯蒂', '英国');
        INSERT INTO books (id, title, author_id, price, published_on) VALUES
            (1, '活着', 1, 45.0, '1993-01-01'),
            (2, '骆驼祥子', 2, 32.0, '1936-01-01'),
            (3, '白夜行', 3, 59.8, '1999-01-01'),
            (4, '东方快车谋杀案', 4, 39.5, '1934-01-01'),
            (5, '许三观卖血记', 1, 42.0, '1995-01-01');
        ",
    )
    .map_err(|err| AppError::msg(err.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{commit_changes, export_table_csv, fetch_rows, import_table_csv};
    use crate::models::{
        CommitRequest, ExportTableRequest, FetchRowsRequest, FieldValue, ImportCsvRequest,
        RowChange,
    };
    use crate::query::execute_sql;
    use crate::schema;
    use serde_json::json;

    fn tunnel_fixture(engine: Engine, insecure: bool) -> (StoredProfile, SessionConfig) {
        let request = SaveConnectionRequest {
            engine,
            host: "db.example.test".into(),
            port: engine.default_port(),
            use_ssl: true,
            ssl_insecure: insecure,
            ssh_enabled: true,
            ..SaveConnectionRequest::default()
        };
        let config = SessionConfig {
            engine,
            host: request.host.clone(),
            port: request.port,
            tls_server_name: None,
            username: String::new(),
            password: String::new(),
            database: String::new(),
            file_path: String::new(),
            use_ssl: true,
            ssl_insecure: insecure,
            ssh_password: String::new(),
            server_mode: String::new(),
            auth_method: String::new(),
            warehouse: String::new(),
            schema_name: String::new(),
            address_map: String::new(),
        };
        (profile_from_request(&request), config)
    }

    #[test]
    fn tunnel_routing_preserves_certificate_identity_and_user_verification_choice() {
        for insecure in [false, true] {
            let (profile, mut config) = tunnel_fixture(Engine::Postgres, insecure);
            validate_tunnel_security(&profile, &config).unwrap();
            apply_tunnel_destination(&mut config, 49152);
            assert_eq!(config.host, "127.0.0.1");
            assert_eq!(config.port, 49152);
            assert_eq!(config.tls_server_name.as_deref(), Some("db.example.test"));
            assert_eq!(config.ssl_insecure, insecure);
            assert!(config.use_ssl);
        }
    }

    #[test]
    fn unsupported_verified_tls_tunnels_are_refused_before_connecting() {
        for engine in [
            Engine::Mysql,
            Engine::Mariadb,
            Engine::Mongodb,
            Engine::Dameng,
            Engine::Redis,
            Engine::Oracle,
            Engine::Snowflake,
        ] {
            let (profile, config) = tunnel_fixture(engine, false);
            assert!(
                validate_tunnel_security(&profile, &config).is_err(),
                "{}",
                engine.label()
            );
        }
        let (profile, config) = tunnel_fixture(Engine::Mysql, true);
        assert!(validate_tunnel_security(&profile, &config).is_ok());
        let (profile, config) = tunnel_fixture(Engine::Mssql, false);
        assert!(validate_tunnel_security(&profile, &config).is_ok());
    }

    #[test]
    fn cluster_target_parsing_keeps_ipv6_host_and_port_separate() {
        assert_eq!(
            split_host_port("[2001:db8::1]:6380", 6379),
            ("2001:db8::1".into(), 6380)
        );
        assert_eq!(
            split_host_port("2001:db8::1", 6379),
            ("2001:db8::1".into(), 6379)
        );
        assert_eq!(
            split_host_port("db.example.test:6380", 6379),
            ("db.example.test".into(), 6380)
        );
    }

    #[test]
    fn concurrent_profile_updates_from_two_instances_preserve_every_connection() {
        let dir = std::env::temp_dir().join(format!("dataharbor-profile-race-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let first = AppState::open_in(dir.clone(), true).unwrap();
            let second = AppState::open_in(dir.clone(), true).unwrap();
            std::thread::scope(|scope| {
                for index in 0..12 {
                    let state = if index % 2 == 0 { &first } else { &second };
                    let path = dir.join(format!("fixture-{index}.sqlite"));
                    scope.spawn(move || {
                        state
                            .save_connection(SaveConnectionRequest {
                                name: format!("connection-{index}"),
                                engine: Engine::Sqlite,
                                file_path: path.display().to_string(),
                                ..Default::default()
                            })
                            .unwrap()
                    });
                }
            });
            let reopened = AppState::open_in(dir.clone(), true).unwrap();
            assert_eq!(reopened.list_connections().len(), 12);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn transport_parameter_changes_retire_the_live_session_but_cosmetic_edits_do_not() {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-reconnect-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        {
            let state = AppState::open_in(directory.clone(), true).unwrap();
            let mut request = SaveConnectionRequest {
                name: "fixture".into(),
                engine: Engine::Sqlite,
                file_path: directory.join("fixture.sqlite").display().to_string(),
                ssh_port: 22,
                http_proxy_port: 8080,
                ..Default::default()
            };
            let saved = state.save_connection(request.clone()).unwrap();
            let id = saved.connection.id;
            request.id = Some(id.clone());
            state.connect(&id).unwrap();
            request.name = "renamed".into();
            let cosmetic = state.save_connection(request.clone()).unwrap();
            assert!(!cosmetic.needs_reconnect);
            assert!(cosmetic.connection.connected);
            let changes: [fn(&mut SaveConnectionRequest); 7] = [
                |r| r.ssh_port = 2222,
                |r| r.ssh_username = "new-user".into(),
                |r| r.ssh_key_path = "new-key".into(),
                |r| r.ssh_password = Some("new-secret".into()),
                |r| r.clear_ssh_password = true,
                |r| r.http_proxy_host = "new-proxy.example".into(),
                |r| r.http_proxy_port = 3128,
            ];
            for change in changes {
                state.connect(&id).unwrap();
                request.ssh_password = None;
                request.clear_ssh_password = false;
                change(&mut request);
                let updated = state.save_connection(request.clone()).unwrap();
                assert!(updated.needs_reconnect);
                assert!(!updated.connection.connected);
                assert!(state.live_config(&id).is_err());
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sqlite_browse_query_edit_and_csv() {
        let dir = std::env::temp_dir().join(format!("dataharbor-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let db_path = dir.join("shop.sqlite");
        let saved = state
            .save_connection(SaveConnectionRequest {
                id: None,
                name: "商店".into(),
                group: "本地".into(),
                color: "#5b8def".into(),
                engine: Engine::Sqlite,
                host: String::new(),
                port: 0,
                username: "reader".into(),
                password: Some("s3cret-密码".into()),
                clear_password: false,
                database: String::new(),
                file_path: db_path.display().to_string(),
                use_ssl: false,
                ssl_insecure: false,
                ..SaveConnectionRequest::default()
            })
            .unwrap();
        let raw = fs::read_to_string(dir.join("connections.json")).unwrap();
        assert!(!raw.contains("s3cret-密码"));
        assert!(saved.connection.has_password);

        let reopened = AppState::open_in(dir.clone(), true).unwrap();
        let listed = reopened.list_connections();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].has_password);
        let id = listed[0].id.clone();

        reopened.connect(&id).unwrap();
        let created = execute_sql(
            &reopened,
            crate::models::ExecuteSqlRequest {
                connection_id: id.clone(),
                database: "main".into(),
                sql: "CREATE TABLE goods (id INTEGER PRIMARY KEY, name TEXT, price REAL);\
                      INSERT INTO goods (name, price) VALUES ('红茶', 18.5);\
                      INSERT INTO goods (name, price) VALUES ('绿茶', 16);"
                    .into(),
                page: 1,
                page_size: 50,
                only_statement: None,
            },
        )
        .unwrap();
        assert!(created.statements.iter().all(|item| item.ok));

        let tables = schema::list_tables(&reopened, &id, "main", "main").unwrap();
        assert!(tables.iter().any(|name| name == "goods"));
        let columns = schema::list_columns(&reopened, &id, "main", "main", "goods").unwrap();
        assert!(columns
            .iter()
            .any(|column| column.primary_key && column.name == "id"));

        let page = fetch_rows(
            &reopened,
            FetchRowsRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: "main".into(),
                table: "goods".into(),
                filters: vec![crate::models::FilterSpec {
                    column: "name".into(),
                    op: "contains".into(),
                    value: Some("茶".into()),
                }],
                sorts: vec![crate::models::SortSpec {
                    column: "price".into(),
                    desc: true,
                }],
                page: 1,
                page_size: 10,
            },
        )
        .unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows[0][1], json!("红茶"));

        commit_changes(
            &reopened,
            CommitRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: "main".into(),
                table: "goods".into(),
                changes: vec![RowChange {
                    kind: "update".into(),
                    original: Some(
                        page.columns
                            .iter()
                            .zip(&page.rows[0])
                            .map(|(column, value)| FieldValue {
                                column: column.name.clone(),
                                value: value.clone(),
                            })
                            .collect(),
                    ),
                    pk: vec![FieldValue {
                        column: "id".into(),
                        value: page.rows[0][0].clone(),
                    }],
                    values: vec![FieldValue {
                        column: "price".into(),
                        value: json!(21.0),
                    }],
                }],
            },
        )
        .unwrap();

        let queried = execute_sql(
            &reopened,
            crate::models::ExecuteSqlRequest {
                connection_id: id.clone(),
                database: "main".into(),
                sql: "SELECT name, price FROM goods WHERE name = '红茶'".into(),
                page: 1,
                page_size: 20,
                only_statement: None,
            },
        )
        .unwrap();
        assert_eq!(queried.statements[0].rows[0][1], json!(21.0));

        let csv_path = dir.join("goods.csv");
        let exported = export_table_csv(
            &reopened,
            ExportTableRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: "main".into(),
                table: "goods".into(),
                filters: Vec::new(),
                sorts: Vec::new(),
                path: csv_path.display().to_string(),
            },
        )
        .unwrap();
        assert_eq!(exported.row_count, 2);
        let csv = fs::read_to_string(&csv_path).unwrap();
        assert!(csv.contains("红茶"));
        assert!(csv.contains("21"));

        let import_path = dir.join("more.csv");
        fs::write(&import_path, "name,price\n乌龙茶,19.5\n").unwrap();
        let imported = import_table_csv(
            &reopened,
            ImportCsvRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: "main".into(),
                table: "goods".into(),
                path: import_path.display().to_string(),
                truncate: false,
            },
        )
        .unwrap();
        assert_eq!(imported.inserted, 1);
        let after = fetch_rows(
            &reopened,
            FetchRowsRequest {
                connection_id: id,
                database: "main".into(),
                schema: "main".into(),
                table: "goods".into(),
                filters: Vec::new(),
                sorts: Vec::new(),
                page: 1,
                page_size: 20,
            },
        )
        .unwrap();
        assert_eq!(after.total, 3);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sqlite_fk_lookup_and_blob_commit() {
        let dir = std::env::temp_dir().join(format!("dataharbor-fk-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let db_path = dir.join("library.sqlite");
        let saved = state
            .save_connection(SaveConnectionRequest {
                id: None,
                name: "馆藏".into(),
                engine: Engine::Sqlite,
                file_path: db_path.display().to_string(),
                ..SaveConnectionRequest::default()
            })
            .unwrap();
        let id = saved.connection.id.clone();
        state.connect(&id).unwrap();
        execute_sql(
            &state,
            crate::models::ExecuteSqlRequest {
                connection_id: id.clone(),
                database: "main".into(),
                sql: "CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT);\
                      CREATE TABLE books (\
                        id INTEGER PRIMARY KEY,\
                        title TEXT,\
                        author_id INTEGER REFERENCES authors(id),\
                        cover BLOB,\
                        blurb TEXT\
                      );\
                      INSERT INTO authors (id, name) VALUES (1, '余华'), (2, '老舍');"
                    .into(),
                page: 1,
                page_size: 20,
                only_statement: None,
            },
        )
        .unwrap();

        let keys = crate::studio::foreign_keys(&state, &id, "main", "main", "books").unwrap();
        let author = keys
            .iter()
            .find(|key| key.column.eq_ignore_ascii_case("author_id"))
            .expect("author_id foreign key");
        assert_eq!(author.ref_table, "authors");
        assert_eq!(author.ref_column.to_ascii_lowercase(), "id");

        let picked =
            crate::studio::reference_rows(&state, &id, "main", "main", "authors", "id", "老")
                .unwrap();
        assert!(picked
            .rows
            .iter()
            .any(|row| row.iter().any(|cell| cell == &json!("老舍"))));
        assert!(!picked
            .rows
            .iter()
            .any(|row| row.iter().any(|cell| cell == &json!("余华"))));

        let cover = crate::sqlutil::json_blob(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
        commit_changes(
            &state,
            CommitRequest {
                connection_id: id.clone(),
                database: "main".into(),
                schema: "main".into(),
                table: "books".into(),
                changes: vec![RowChange {
                    kind: "insert".into(),
                    original: None,
                    pk: Vec::new(),
                    values: vec![
                        FieldValue {
                            column: "id".into(),
                            value: json!(1),
                        },
                        FieldValue {
                            column: "title".into(),
                            value: json!("活着"),
                        },
                        FieldValue {
                            column: "author_id".into(),
                            value: json!(1),
                        },
                        FieldValue {
                            column: "cover".into(),
                            value: cover.clone(),
                        },
                        FieldValue {
                            column: "blurb".into(),
                            value: json!("<p>余华</p>"),
                        },
                    ],
                }],
            },
        )
        .unwrap();

        let rows = fetch_rows(
            &state,
            FetchRowsRequest {
                connection_id: id,
                database: "main".into(),
                schema: "main".into(),
                table: "books".into(),
                filters: Vec::new(),
                sorts: Vec::new(),
                page: 1,
                page_size: 10,
            },
        )
        .unwrap();
        assert_eq!(rows.rows[0][3], cover);
        let _ = fs::remove_dir_all(&dir);
    }
}
