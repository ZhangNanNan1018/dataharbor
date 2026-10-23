use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    Message,
    Cancelled,
    OperationMissing,
    PartialWrite,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub kind: ErrorKind,
    pub message: String,
    pub committed: Option<u64>,
    pub batches: Option<u64>,
    pub applied: Option<usize>,
    pub failed_index: Option<usize>,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Message(String),
    #[error("操作已取消；已知已提交 {committed} 行、{batches} 批，已提交内容不会自动回滚。原生还原或被中断语句的数据库结果需另行核对")]
    Cancelled { committed: u64, batches: u64 },
    #[error("任务不存在或状态记录已释放，请核对操作结果")]
    OperationMissing,
    #[error("第 {failed_index} 条更改失败，此前 {applied} 条已提交，失败条目的结果可能需要核对；请刷新并核对目标数据后继续，勿重放整批更改：{detail}")]
    PartialWrite {
        applied: usize,
        failed_index: usize,
        detail: String,
    },
}

impl AppError {
    pub fn msg(text: impl Into<String>) -> Self {
        AppError::Message(text.into())
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::Cancelled { .. } => "cancelled",
            Self::OperationMissing => "operationMissing",
            Self::PartialWrite { .. } => "partialWrite",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut payload = ErrorPayload {
            kind: ErrorKind::Message,
            message: self.to_string(),
            committed: None,
            batches: None,
            applied: None,
            failed_index: None,
        };
        match self {
            Self::Message(_) => {}
            Self::OperationMissing => payload.kind = ErrorKind::OperationMissing,
            Self::Cancelled { committed, batches } => {
                payload.kind = ErrorKind::Cancelled;
                payload.committed = Some(*committed);
                payload.batches = Some(*batches);
            }
            Self::PartialWrite {
                applied,
                failed_index,
                ..
            } => {
                payload.kind = ErrorKind::PartialWrite;
                payload.applied = Some(*applied);
                payload.failed_index = Some(*failed_index);
            }
        }
        payload.serialize(serializer)
    }
}
