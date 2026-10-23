//! Foreground operation ownership, observable progress and cooperative cancellation.
//! Cancellation signals are scoped to a held driver session, never a global connection ID.
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

pub type CancelHandle = Arc<dyn Fn() -> AppResult<()> + Send + Sync>;

/// File-producing operations publish once; cancellation/failure leaves the old file intact.
pub(crate) struct AtomicOutput {
    target: std::path::PathBuf,
    temporary: std::path::PathBuf,
}
impl AtomicOutput {
    pub(crate) fn new(target: &std::path::Path) -> AppResult<Self> {
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent).map_err(|err| AppError::msg(err.to_string()))?;
        let name = target
            .file_name()
            .ok_or_else(|| AppError::msg("请选择输出文件"))?;
        let temporary = parent.join(format!(
            ".{}.{}.tmp",
            name.to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&temporary)
            .map_err(|err| AppError::msg(err.to_string()))?;
        Ok(Self {
            target: target.into(),
            temporary,
        })
    }
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.temporary
    }
    pub(crate) fn commit(self) -> AppResult<()> {
        std::fs::rename(&self.temporary, &self.target)
            .map_err(|err| AppError::msg(format!("发布输出文件失败：{err}")))
    }
}
impl Drop for AtomicOutput {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temporary);
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct OperationSnapshot {
    pub id: String,
    pub label: String,
    pub status: String,
    pub phase: String,
    pub completed: u64,
    #[cfg_attr(test, ts(optional = nullable))]
    pub total: Option<u64>,
    pub committed: u64,
    pub batches: u64,
    pub cancel_requested: bool,
    pub message: String,
    pub interruptible: bool,
}

struct InterruptSlot {
    generation: u64,
    handle: Option<CancelHandle>,
}
struct Inner {
    journal: Mutex<Option<crate::run_history::RunLease>>,
    reported_failure: Mutex<Option<String>>,
    progress: Mutex<OperationSnapshot>,
    cancelled: AtomicBool,
    interrupt: Mutex<InterruptSlot>,
    finished: Mutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct OperationContext {
    inner: Arc<Inner>,
}

impl OperationContext {
    fn new(id: String, label: String, status: &str) -> Self {
        Self {
            inner: Arc::new(Inner {
                journal: Mutex::new(None),
                reported_failure: Mutex::new(None),
                progress: Mutex::new(OperationSnapshot {
                    id,
                    label,
                    status: status.into(),
                    phase: "准备中".into(),
                    completed: 0,
                    total: None,
                    committed: 0,
                    batches: 0,
                    cancel_requested: false,
                    message: String::new(),
                    interruptible: false,
                }),
                cancelled: AtomicBool::new(false),
                interrupt: Mutex::new(InterruptSlot {
                    generation: 0,
                    handle: None,
                }),
                finished: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn abandon_record(&self) {
        self.inner.journal.lock().unwrap_or_else(|p| p.into_inner()).take();
    }

    pub(crate) fn record(&self, history: &crate::run_history::RunHistory, job: Option<(&str, &str)>) -> AppResult<()> {
        let lease = history.start(&self.snapshot().id, job)?;
        *self.inner.journal.lock().unwrap_or_else(|p| p.into_inner()) = Some(lease);
        Ok(())
    }

    pub fn detached() -> Self {
        Self::new(uuid::Uuid::new_v4().to_string(), String::new(), "running")
    }
    pub fn snapshot(&self) -> OperationSnapshot {
        self.inner
            .progress
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub fn cancel_requested(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }
    pub fn cancel_error(&self) -> AppError {
        let state = self.snapshot();
        AppError::Cancelled {
            committed: state.committed,
            batches: state.batches,
        }
    }
    pub fn checkpoint(&self) -> AppResult<()> {
        if self.cancel_requested() {
            Err(self.cancel_error())
        } else {
            Ok(())
        }
    }
    pub fn report(&self, phase: &str, completed: u64, total: Option<u64>, message: &str) {
        let mut state = self
            .inner
            .progress
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if is_terminal(&state.status) {
            return;
        }
        state.phase = phase.into();
        state.completed = completed;
        state.total = total;
        state.message = message.into();
    }
    pub fn committed(&self, rows: u64) {
        let mut state = self
            .inner
            .progress
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        state.committed = state.committed.saturating_add(rows);
        state.batches = state.batches.saturating_add(1);
        if let Some(lease) = self.inner.journal.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            if lease.progress(&state.phase, state.committed, state.batches).is_err() {
                state.message.push_str("；提交进度记录失败，异常退出后需核对目标数据");
            }
        }
    }
    pub fn report_failure(&self, message: String) {
        *self
            .inner
            .reported_failure
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(message);
    }

    pub fn bind_interrupt(&self, handle: Option<CancelHandle>) -> AppResult<InterruptGuard> {
        self.checkpoint()?;
        let mut slot = self
            .inner
            .interrupt
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // The worker still owns the session here, so a concurrent cancellation
        // cannot signal a later unrelated statement on that session.
        self.checkpoint()?;
        slot.generation += 1;
        slot.handle = handle;
        let generation = slot.generation;
        self.inner
            .progress
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .interruptible = slot.handle.is_some();
        Ok(InterruptGuard {
            operation: self.clone(),
            generation,
        })
    }

    pub fn request_cancel(&self) -> AppResult<OperationSnapshot> {
        {
            let mut state = self
                .inner
                .progress
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if is_terminal(&state.status) {
                return Ok(state.clone());
            }
            if state.cancel_requested {
                return Ok(state.clone());
            }
            self.inner.cancelled.store(true, Ordering::Release);
            state.cancel_requested = true;
            if state.message.is_empty() {
                state.message = "已请求取消，正在等待当前操作安全停止".into();
            }
            if state.status == "queued" {
                state.status = "cancelled".into();
                *self
                    .inner
                    .finished
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
            }
        }
        let slot = self
            .inner
            .interrupt
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut repeat = None;
        if let Some(signal) = &slot.handle {
            if let Err(error) = signal() {
                // The cancellation flag remains set even if a driver cannot
                // interrupt immediately. The next safe boundary still stops.
                self.inner
                    .progress
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .message = format!("取消已请求；当前操作尚未中断，等待安全边界：{error}");
            } else {
                repeat = Some(slot.generation);
            }
        }
        drop(slot);
        if let Some(generation) = repeat {
            // A driver can receive the first signal just before execute() starts.
            // Retry only while this exact lease remains installed; retiring the
            // lease synchronizes with each signal before the session is reused.
            let context = self.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(40));
                let slot = context
                    .inner
                    .interrupt
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if slot.generation != generation {
                    break;
                }
                let Some(signal) = &slot.handle else {
                    break;
                };
                if signal().is_err() {
                    break;
                }
            });
        }
        Ok(self.snapshot())
    }

    pub fn finish<T>(&self, result: &AppResult<T>) {
        let mut slot = self
            .inner
            .interrupt
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        slot.handle = None;
        let mut state = self
            .inner
            .progress
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        state.interruptible = false;
        let last_phase = state.phase.clone();
        match result {
            Ok(_) => {
                if let Some(error) = self
                    .inner
                    .reported_failure
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .as_ref()
                {
                    state.status = "failed".into();
                    state.phase = "失败".into();
                    state.message = error.clone();
                } else {
                    state.status = "succeeded".into();
                    state.phase = "已完成".into();
                }
            }
            Err(error) => {
                let detail = state.message.clone();
                state.status = if matches!(error, AppError::Cancelled { .. }) {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                state.phase = if state.status == "cancelled" {
                    "已取消"
                } else {
                    "失败"
                }
                .into();
                state.message = error.to_string();
                if state.status == "cancelled" && !detail.is_empty() {
                    state.message.push_str(&format!("。{detail}"));
                }
            }
        }
        if let Some(lease) = self.inner.journal.lock().unwrap_or_else(|p| p.into_inner()).take() {
            if lease.finish(&state.status, state.committed, state.batches, &last_phase, result.as_ref().err().map(AppError::kind).or_else(|| (state.status == "failed").then_some("statementError"))).is_err() {
                state.message.push_str("；执行记录未能保存，重启后请核对数据库或输出文件");
            }
        }
        *self
            .inner
            .finished
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    }
}

pub struct InterruptGuard {
    operation: OperationContext,
    generation: u64,
}
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        let mut slot = self
            .operation
            .inner
            .interrupt
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if slot.generation == self.generation {
            slot.handle = None;
            self.operation
                .inner
                .progress
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .interruptible = false;
        }
    }
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "succeeded" | "failed" | "cancelled")
}

#[derive(Default)]
pub struct OperationManager {
    entries: Mutex<HashMap<String, OperationContext>>,
}
impl OperationManager {
    pub fn create(&self, label: String) -> OperationSnapshot {
        let context = OperationContext::new(
            uuid::Uuid::new_v4().to_string(),
            label.chars().take(120).collect(),
            "queued",
        );
        let snapshot = context.snapshot();
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|_, entry| {
            !entry
                .inner
                .finished
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some_and(|time| time.elapsed() > Duration::from_secs(300))
        });
        entries.insert(snapshot.id.clone(), context);
        snapshot
    }
    pub fn get(&self, id: &str) -> AppResult<OperationContext> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
            .ok_or(AppError::OperationMissing)
    }
    pub fn claim(&self, id: &str) -> AppResult<OperationContext> {
        let operation = self.get(id)?;
        {
            let mut state = operation
                .inner
                .progress
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if state.status == "cancelled" {
                drop(state);
                return Err(operation.cancel_error());
            }
            if state.status != "queued" {
                return Err(AppError::msg("任务已开始或已结束，不能重复执行"));
            }
            state.status = "running".into();
        }
        Ok(operation)
    }
    pub fn release(&self, id: &str) -> AppResult<()> {
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(context) = entries.get(id) {
            if !is_terminal(&context.snapshot().status) {
                return Err(AppError::msg("任务仍在运行，请等待实际停止后关闭"));
            }
        }
        entries.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn cancellation_before_claim_prevents_work_and_duplicate_claims_are_rejected() {
        let manager = OperationManager::default();
        let first = manager.create("同步".into());
        manager.get(&first.id).unwrap().request_cancel().unwrap();
        assert!(matches!(
            manager.claim(&first.id),
            Err(AppError::Cancelled { committed: 0, .. })
        ));
        manager.release(&first.id).unwrap();
        let second = manager.create("查询".into());
        let op = manager.claim(&second.id).unwrap();
        assert!(manager.claim(&second.id).is_err());
        assert!(manager.release(&second.id).is_err());
        op.finish(&Ok(()));
        manager.release(&second.id).unwrap();
    }

    #[test]
    fn cancellation_keeps_committed_progress_and_stale_hooks_cannot_signal_new_work() {
        let op = OperationContext::detached();
        let calls = Arc::new(AtomicUsize::new(0));
        let signal_calls = calls.clone();
        let guard = op
            .bind_interrupt(Some(Arc::new(move || {
                signal_calls.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })))
            .unwrap();
        op.committed(500);
        op.request_cancel().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        drop(guard);
        op.request_cancel().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let result: AppResult<()> = op.checkpoint();
        assert!(matches!(
            result,
            Err(AppError::Cancelled {
                committed: 500,
                batches: 1
            })
        ));
        op.finish(&result);
        assert_eq!(op.snapshot().status, "cancelled");
        assert_eq!(op.snapshot().committed, 500);
    }

    #[test]
    fn cancellation_retries_a_signal_before_execute_but_stops_when_lease_retires() {
        let op = OperationContext::detached();
        let calls = Arc::new(AtomicUsize::new(0));
        let signals = calls.clone();
        let guard = op
            .bind_interrupt(Some(Arc::new(move || {
                signals.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })))
            .unwrap();
        op.request_cancel().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while calls.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(calls.load(Ordering::SeqCst) >= 2);
        drop(guard);
        let retired = calls.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(90));
        assert_eq!(calls.load(Ordering::SeqCst), retired);
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    #[test]
    fn cancelled_output_is_removed_without_replacing_existing_file() {
        let directory =
            std::env::temp_dir().join(format!("dataharbor-output-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let target = directory.join("result.csv");
        std::fs::write(&target, "original").unwrap();
        {
            let output = AtomicOutput::new(&target).unwrap();
            std::fs::write(output.path(), "partial").unwrap();
            // Failure/cancellation drops the unpublished output.
        }
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
