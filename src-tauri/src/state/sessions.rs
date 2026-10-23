//! A query console owns one physical session. Its scripts cannot interleave with
//! another console, catalog reads, or a scheduled job. Tunnels outlive every lease.
use super::{lock, AppState};
use crate::drivers::{self, DbSession};
use crate::error::{AppError, AppResult};
use crate::models::SessionConfig;
use crate::operations::OperationContext;
use crate::tunnel::TunnelGuard;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, TryLockError, Weak};

pub struct SessionSlot {
    pub inner: Mutex<Box<dyn DbSession>>,
    pub tunnel: Vec<TunnelGuard>,
    pub live: SessionConfig,
}

impl SessionSlot {
    pub fn tunnel_count(&self) -> usize {
        self.tunnel.len()
    }
}

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

    fn session_owner(&self, id: &str) -> AppResult<Arc<SessionSlot>> {
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
        let owner = self.session_owner(id)?;
        let console = {
            let mut sessions = lock(&self.query_sessions.0);
            sessions
                .entry(session_id.into())
                .or_insert_with(|| {
                    Arc::new(Console {
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
        if guard.is_none() {
            let inner = drivers::open_session(&owner.live)?;
            *guard = Some(IsolatedSession {
                inner,
                _owner: owner,
            });
        }
        operation.checkpoint()?;
        action(&mut **guard.as_mut().expect("initialized console"))
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
        let inner = drivers::open_session(&owner.live)?;
        Ok(IsolatedSession {
            inner,
            _owner: owner,
        })
    }
}
