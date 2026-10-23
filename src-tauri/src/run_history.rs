//! Durable operation metadata. Per-run OS leases distinguish a crashed process
//! from another live desktop/CLI instance; SQL, credentials and row data are absent.
use crate::error::{AppError, AppResult};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunRecord {
    pub id: String,
    pub kind: String,
    #[cfg_attr(test, ts(optional = nullable))]
    pub job_id: Option<String>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub revision: Option<String>,
    pub status: String,
    pub phase: String,
    pub error_kind: Option<String>,
    pub started_at: u64,
    #[cfg_attr(test, ts(optional = nullable))]
    pub finished_at: Option<u64>,
    pub committed: u64,
    pub batches: u64,
}

#[derive(Clone)]
pub(crate) struct RunHistory {
    directory: PathBuf,
}
pub(crate) struct RunLease {
    history: RunHistory,
    id: String,
    _lock: File,
}

impl RunHistory {
    pub fn open(directory: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(&directory).map_err(error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).map_err(error)?;
        }
        let history = Self { directory };
        let db = history.connect()?;
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(error)?;
        if version > 1 {
            return Err(AppError::msg("执行记录版本较新，请使用兼容版本打开"));
        }
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS runs(
              id TEXT PRIMARY KEY, kind TEXT NOT NULL, job_id TEXT, revision TEXT,
              status TEXT NOT NULL, phase TEXT NOT NULL DEFAULT '准备中', error_kind TEXT, started_at INTEGER NOT NULL, finished_at INTEGER,
              committed INTEGER NOT NULL DEFAULT 0, batches INTEGER NOT NULL DEFAULT 0);
            PRAGMA user_version=1;").map_err(error)?;
        history.recover()?;
        Ok(history)
    }

    fn connect(&self) -> AppResult<Connection> {
        let db = Connection::open(self.directory.join("history.sqlite")).map_err(error)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(error)?;
        Ok(db)
    }

    fn lease_file(&self, id: &str) -> AppResult<File> {
        uuid::Uuid::parse_str(id).map_err(|_| AppError::msg("执行记录标识无效"))?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join(format!("{id}.lock")))
            .map_err(error)
    }

    pub fn start(&self, id: &str, job: Option<(&str, &str)>) -> AppResult<RunLease> {
        let file = self.lease_file(id)?;
        file.try_lock().map_err(error)?;
        let (job_id, revision) =
            job.map_or((None, None), |(id, revision)| (Some(id), Some(revision)));
        self.connect()?.execute("INSERT INTO runs(id,kind,job_id,revision,status,started_at) VALUES(?1,?2,?3,?4,'running',?5)",
            params![id, if job.is_some() {"scheduled"} else {"foreground"}, job_id, revision, now()]).map_err(error)?;
        Ok(RunLease {
            history: self.clone(),
            id: id.into(),
            _lock: file,
        })
    }

    fn recover(&self) -> AppResult<()> {
        let db = self.connect()?;
        let mut query = db
            .prepare("SELECT id FROM runs WHERE status='running'")
            .map_err(error)?;
        let ids = query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        drop(query);
        for id in ids {
            let file = self.lease_file(&id)?;
            match file.try_lock() {
                Ok(()) => {
                    db.execute("UPDATE runs SET status='unknown',finished_at=?2 WHERE id=?1 AND status='running'", params![id,now()]).map_err(error)?;
                }
                Err(fs::TryLockError::WouldBlock) => {}
                Err(fs::TryLockError::Error(value)) => return Err(error(value)),
            }
        }
        Ok(())
    }

    pub fn list(&self) -> AppResult<Vec<RunRecord>> {
        self.recover()?;
        self.select("SELECT id,kind,job_id,revision,status,started_at,finished_at,committed,batches,phase,error_kind FROM runs ORDER BY started_at DESC,rowid DESC LIMIT 200")
    }

    pub fn uncertain_jobs(&self) -> AppResult<Vec<RunRecord>> {
        self.recover()?;
        self.select("SELECT id,kind,job_id,revision,status,started_at,finished_at,committed,batches,phase,error_kind FROM runs WHERE kind='scheduled' AND status='unknown'")
    }

    fn select(&self, sql: &str) -> AppResult<Vec<RunRecord>> {
        let db = self.connect()?;
        let mut query = db.prepare(sql).map_err(error)?;
        let records = query
            .query_map([], |row| {
                Ok(RunRecord {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    job_id: row.get(2)?,
                    revision: row.get(3)?,
                    status: row.get(4)?,
                    started_at: row.get(5)?,
                    finished_at: row.get(6)?,
                    committed: row.get(7)?,
                    batches: row.get(8)?,
                    phase: row.get(9)?,
                    error_kind: row.get(10)?,
                })
            })
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        Ok(records)
    }
}

impl RunLease {
    pub fn progress(&self, phase: &str, committed: u64, batches: u64) -> AppResult<()> {
        self.history
            .connect()?
            .execute(
                "UPDATE runs SET phase=?2,committed=?3,batches=?4 WHERE id=?1 AND status='running'",
                params![self.id, phase, committed, batches],
            )
            .map_err(error)?;
        Ok(())
    }
    pub fn finish(
        self,
        status: &str,
        committed: u64,
        batches: u64,
        phase: &str,
        error_kind: Option<&str>,
    ) -> AppResult<()> {
        self.history.connect()?.execute("UPDATE runs SET status=?2,finished_at=?3,committed=?4,batches=?5,phase=?6,error_kind=?7 WHERE id=?1", params![self.id,status,now(),committed,batches,phase,error_kind]).map_err(error)?;
        // The lease is deliberately released only after the durable terminal row.
        Ok(())
    }
}

impl Drop for RunLease {
    fn drop(&mut self) {
        let _ = self._lock.unlock();
        let _ = fs::remove_file(self.history.directory.join(format!("{}.lock", self.id)));
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn error(value: impl std::fmt::Display) -> AppError {
    AppError::msg(format!("执行记录保存失败：{value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_leases_survive_other_instances_and_crashes_become_unknown() {
        let dir = std::env::temp_dir().join(format!("dataharbor-history-{}", uuid::Uuid::new_v4()));
        let first = RunHistory::open(dir.clone()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let lease = first.start(&id, Some(("job", "revision"))).unwrap();
        let second = RunHistory::open(dir.clone()).unwrap();
        assert_eq!(second.list().unwrap()[0].status, "running");
        drop(lease); // Simulate process death without writing a terminal record.
        assert_eq!(second.uncertain_jobs().unwrap()[0].status, "unknown");
        let id = uuid::Uuid::new_v4().to_string();
        first
            .start(&id, None)
            .unwrap()
            .finish("succeeded", 17, 2, "写入", None)
            .unwrap();
        let record = second
            .list()
            .unwrap()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert_eq!(
            (record.status.as_str(), record.committed, record.batches),
            ("succeeded", 17, 2)
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
