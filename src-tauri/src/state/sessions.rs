//! A query console owns one physical session. Its scripts cannot interleave with
//! another console, catalog reads, or a scheduled job. Tunnels outlive every lease.
use super::{lock, AppState};
use crate::drivers::{self, DbSession};
use crate::error::{AppError, AppResult};
use crate::models::{Engine, SessionConfig, StoredProfile};
use crate::operations::OperationContext;
use crate::tunnel::TunnelGuard;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError, Weak};

pub(crate) struct Transport {
    pub(crate) live: SessionConfig,
    tunnels: Vec<TunnelGuard>,
}

impl Transport {
    fn open_session(self: &Arc<Self>, database: &str) -> AppResult<IsolatedSession> {
        let mut config = self.live.clone();
        if !database.is_empty()
            && matches!(
                config.engine,
                Engine::Mysql | Engine::Mariadb | Engine::Postgres | Engine::Mssql
            )
        {
            config.database = database.into();
        }
        let inner = drivers::open_session(&config).map_err(AppError::before_send)?;
        Ok(IsolatedSession {
            inner,
            _transport: self.clone(),
            broken: false,
        })
    }
}

pub(super) struct SessionSlot {
    pub(super) inner: Mutex<IsolatedSession>,
    transport: Mutex<Arc<Transport>>,
    profile: StoredProfile,
    config: SessionConfig,
    pub(super) retired: AtomicBool,
}

impl SessionSlot {
    pub(super) fn new(profile: StoredProfile, config: SessionConfig) -> AppResult<Self> {
        let (transport, inner) = Self::open_transport(&profile, &config, "")?;
        Ok(Self {
            inner: Mutex::new(inner),
            transport: Mutex::new(transport),
            profile,
            config,
            retired: AtomicBool::new(false),
        })
    }

    fn open_transport(
        profile: &StoredProfile,
        config: &SessionConfig,
        database: &str,
    ) -> AppResult<(Arc<Transport>, IsolatedSession)> {
        let mut live = config.clone();
        let tunnels = super::open_with_tunnels(profile, &mut live)?;
        let transport = Arc::new(Transport { live, tunnels });
        let session = transport.open_session(database)?;
        Ok((transport, session))
    }

    pub(super) fn transport(&self) -> Arc<Transport> {
        lock(&self.transport).clone()
    }

    pub(super) fn check_active(&self) -> AppResult<()> {
        if self.retired.load(Ordering::Acquire) {
            return Err(AppError::msg("连接已关闭或配置已更改，排队的请求未执行"));
        }
        Ok(())
    }

    fn open(&self, database: &str) -> AppResult<IsolatedSession> {
        self.check_active()?;
        let transport = self.transport();
        let session = match transport.open_session(database) {
            Ok(session) => session,
            Err(e) if e.disconnect_phase().is_some() && !transport.tunnels.is_empty() => {
                let mut current = lock(&self.transport);
                self.check_active()?;
                if Arc::ptr_eq(&transport, &current) {
                    let (next, session) =
                        Self::open_transport(&self.profile, &self.config, database)?;
                    *current = next;
                    session
                } else {
                    current.open_session(database)?
                }
            }
            Err(e) => return Err(e.before_send()),
        };
        self.check_active()?;
        Ok(session)
    }

    pub(super) fn read<T>(
        &self,
        session: &mut IsolatedSession,
        database: &str,
        action: impl FnOnce(&mut dyn DbSession) -> AppResult<T>,
    ) -> AppResult<T> {
        self.check_active()?;
        if !session.broken {
            if let Err(e) = session.probe(database) {
                if e.disconnect_phase().is_none() {
                    return Err(e);
                }
                session.broken = true;
            }
        }
        if session.broken {
            let mut next = self.open("")?;
            next.probe(database).map_err(AppError::before_send)?;
            *session = next;
        }
        self.check_active()?;
        let result = action(&mut **session);
        if self.retired.load(Ordering::Acquire) {
            return Err(AppError::driver("读取所属的连接已关闭", true));
        }
        if result
            .as_ref()
            .is_err_and(|e| e.disconnect_phase().is_some())
        {
            session.broken = true;
        }
        result
    }

    pub(super) fn tunnel_count(&self) -> usize {
        self.transport().tunnels.len()
    }
}

pub(crate) struct IsolatedSession {
    inner: Box<dyn DbSession>,
    _transport: Arc<Transport>,
    broken: bool,
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

/// Serialize connection configuration changes and handshakes only for that ID.
#[derive(Default)]
pub(super) struct ConnectionLocks(Mutex<HashMap<String, Weak<Mutex<()>>>>);
impl ConnectionLocks {
    pub(super) fn for_id(&self, id: &str) -> Arc<Mutex<()>> {
        let mut locks = lock(&self.0);
        locks.retain(|_, entry| entry.strong_count() > 0);
        if let Some(existing) = locks.get(id).and_then(Weak::upgrade) {
            return existing;
        }
        let value = Arc::new(Mutex::new(()));
        locks.insert(id.into(), Arc::downgrade(&value));
        value
    }
}

struct Console {
    disconnected: AtomicBool,
    retired: AtomicBool,
    connection_id: String,
    database: String,
    owner: Arc<SessionSlot>,
    session: Mutex<Option<IsolatedSession>>,
}

#[derive(Default)]
pub(super) struct QuerySessions(Mutex<HashMap<String, Arc<Console>>>);
impl QuerySessions {
    pub(super) fn remove_connection(&self, id: &str) {
        let removed = {
            let mut sessions = lock(&self.0);
            let removed: Vec<_> = sessions
                .values()
                .filter(|console| console.connection_id == id)
                .cloned()
                .collect();
            for console in &removed {
                console.retired.store(true, Ordering::Release);
            }
            sessions.retain(|_, console| console.connection_id != id);
            removed
        };
        drop(removed);
    }
}

impl AppState {
    pub(crate) fn check_query_generation(&self, key: Option<&str>) -> AppResult<()> {
        if let Some(key) = key.filter(|id| id.starts_with("tab:")) {
            let sessions = lock(&self.query_sessions.0);
            if !sessions.get(key).is_some_and(|c| {
                !c.retired.load(Ordering::Acquire) && !c.owner.retired.load(Ordering::Acquire)
            }) {
                return Err(AppError::SessionExpired);
            }
        }
        Ok(())
    }

    pub(crate) fn query_session_lost(&self, session_id: Option<&str>) {
        if let Some(console) =
            session_id.and_then(|id| lock(&self.query_sessions.0).get(id).cloned())
        {
            console.disconnected.store(true, Ordering::Release);
        }
    }

    pub(crate) fn close_query_session(&self, session_id: &str) {
        // An in-flight script retains its lease until it stops. Dropping the last
        // lease closes its connection (and rolls back an unfinished transaction).
        let removed = {
            let mut sessions = lock(&self.query_sessions.0);
            let removed = sessions.remove(session_id);
            if let Some(console) = &removed {
                console.retired.store(true, Ordering::Release);
            }
            removed
        };
        drop(removed);
    }

    pub(super) fn session_owner(&self, id: &str) -> AppResult<Arc<SessionSlot>> {
        lock(&self.sessions)
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::msg("连接未打开，请先在左侧连接"))
    }

    pub(crate) fn with_query_session<T>(
        &self,
        id: &str,
        database: &str,
        session_id: Option<&str>,
        operation: &OperationContext,
        action: impl FnOnce(&mut dyn DbSession) -> AppResult<T>,
    ) -> AppResult<T> {
        operation.checkpoint()?;
        let Some(session_id) = session_id else {
            let mut session = self.open_isolated_session(id)?;
            return action(&mut *session);
        };
        if session_id.is_empty() || session_id.len() > 256 {
            return Err(AppError::msg("查询会话标识无效"));
        }
        let owner = self.session_owner(id).map_err(|e| {
            if session_id.starts_with("tab:") {
                AppError::SessionExpired
            } else {
                e
            }
        })?;
        let console = {
            let mut sessions = lock(&self.query_sessions.0);
            owner.check_active()?;
            if session_id.starts_with("tab:") && !sessions.contains_key(session_id) {
                return Err(AppError::SessionExpired);
            }
            sessions
                .entry(session_id.into())
                .or_insert_with(|| {
                    Arc::new(Console {
                        disconnected: AtomicBool::new(false),
                        retired: AtomicBool::new(false),
                        connection_id: id.into(),
                        database: database.into(),
                        owner: owner.clone(),
                        session: Mutex::new(None),
                    })
                })
                .clone()
        };
        if console.connection_id != id
            || console.database != database
            || !Arc::ptr_eq(&console.owner, &owner)
        {
            return Err(AppError::msg(
                "查询会话的连接已变化；请重新打开查询标签，原事务不会迁移到新连接",
            ));
        }
        let mut guard = loop {
            operation.checkpoint()?;
            match console.session.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::Poisoned(error)) => break error.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    operation.report("等待查询会话", 0, None, "同一标签的上一项操作尚未结束");
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
            }
        };
        if console.retired.load(Ordering::Acquire) {
            return Err(AppError::msg("查询会话已关闭，排队的语句未执行"));
        }
        if console.disconnected.load(Ordering::Acquire) {
            return Err(
                AppError::driver("此标签的连接已中断", true).before_send(),
            );
        }
        if guard.is_none() {
            *guard = Some(owner.open(database)?);
        }
        operation.checkpoint()?;
        owner.check_active()?;
        let session = guard.as_mut().expect("initialized console");
        let result = action(&mut **session);
        if session.engine().capabilities().persistent_console
            && result
                .as_ref()
                .is_err_and(|e| e.disconnect_phase().is_some())
        {
            console.disconnected.store(true, Ordering::Release);
        }
        if console.retired.load(Ordering::Acquire) || owner.retired.load(Ordering::Acquire) {
            return Err(AppError::driver("请求所属的标签会话已关闭", true));
        }
        result
    }

    /// A managed ID names exactly one generation. Execution can never recreate it.
    #[cfg(test)]
    pub(crate) fn reconnect_query_session(
        &self,
        id: &str,
        database: &str,
        previous: Option<&str>,
    ) -> AppResult<String> {
        self.prepare_query_session(id, database, previous, false)
    }

    pub(crate) fn prepare_query_session(
        &self,
        id: &str,
        database: &str,
        previous: Option<&str>,
        reuse_healthy: bool,
    ) -> AppResult<String> {
        let owner = self.session_owner(id)?;
        let old = previous.and_then(|key| lock(&self.query_sessions.0).get(key).cloned());
        if previous.is_some() && old.is_none() {
            return Err(AppError::SessionExpired);
        }
        if let Some(old) = &old {
            if !Arc::ptr_eq(&owner, &old.owner) || old.database != database {
                return Err(AppError::msg("标签所属连接已改变"));
            }
        }
        let mut idle = old
            .as_ref()
            .map(|old| {
                old.session
                    .try_lock()
                    .map_err(|_| AppError::msg("标签正在执行操作"))
            })
            .transpose()?;
        if let (true, Some(old), Some(guard), Some(key)) = (reuse_healthy, &old, &mut idle, previous) {
            if old.retired.load(Ordering::Acquire) {
                return Err(AppError::SessionExpired);
            }
            if !old.disconnected.load(Ordering::Acquire) {
                if let Some(session) = guard.as_mut() {
                    match session.probe(database) {
                        Ok(()) => {
                            self.check_query_generation(Some(key))?;
                            return Ok(key.to_owned());
                        }
                        Err(e) if e.disconnect_phase().is_some() => {
                            old.disconnected.store(true, Ordering::Release);
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
        }
        let mut session = owner.open(database)?;
        session.probe(database).map_err(AppError::before_send)?;
        let key = format!("tab:{}", uuid::Uuid::new_v4());
        let mut sessions = lock(&self.query_sessions.0);
        owner.check_active()?;
        if let (Some(previous), Some(old)) = (previous, &old) {
            if old.retired.load(Ordering::Acquire) {
                return Err(AppError::msg("查询会话已关闭"));
            }
            old.retired.store(true, Ordering::Release);
            sessions.remove(previous);
        }
        sessions.insert(
            key.clone(),
            Arc::new(Console {
                disconnected: AtomicBool::new(false),
                retired: AtomicBool::new(false),
                connection_id: id.into(),
                database: database.into(),
                owner,
                session: Mutex::new(Some(session)),
            }),
        );
        Ok(key)
    }

    pub(crate) fn with_read_session<T>(
        &self,
        id: &str,
        database: &str,
        action: impl FnOnce(&mut dyn DbSession) -> AppResult<T>,
    ) -> AppResult<T> {
        let slot = self.session_owner(id)?;
        let mut session = lock(&slot.inner);
        slot.read(&mut session, database, action)
    }

    pub(crate) fn open_connected_session(&self, id: &str) -> AppResult<IsolatedSession> {
        self.session_owner(id)?.open("")
    }

    pub(crate) fn open_isolated_session(&self, id: &str) -> AppResult<IsolatedSession> {
        let existing = { lock(&self.sessions).get(id).cloned() };
        let owner = match existing {
            Some(owner) => owner,
            None => {
                self.connect(id)?;
                self.session_owner(id)?
            }
        };
        owner.open("")
    }
}

#[cfg(test)]
mod recovery_tests;
