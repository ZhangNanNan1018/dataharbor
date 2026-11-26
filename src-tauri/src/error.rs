use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum DisconnectPhase {
    BeforeSend,
    OutcomeUnknown,
}

impl std::fmt::Display for DisconnectPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BeforeSend => "发送前连接失败，本次请求未执行",
            Self::OutcomeUnknown => "连接中断，执行结果未知，请核对数据库后再操作",
        })
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    Message,
    Cancelled,
    OperationMissing,
    PartialWrite,
    ConnectionLost,
    SessionExpired,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub kind: ErrorKind,
    #[cfg_attr(test, ts(optional = nullable))]
    pub disconnect_phase: Option<DisconnectPhase>,
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
    #[error("标签原会话已关闭，本次请求未执行")]
    SessionExpired,
    #[error("{phase}：{detail}")]
    ConnectionLost {
        phase: DisconnectPhase,
        detail: String,
    },
    #[error("操作已取消；已知已提交 {committed} 行、{batches} 批，已提交内容不会自动回滚。原生还原或被中断语句的数据库结果需另行核对")]
    Cancelled { committed: u64, batches: u64 },
    #[error("任务不存在或状态记录已释放，请核对操作结果")]
    OperationMissing,
    #[error("第 {failed_index} 条更改失败，此前 {applied} 条已提交，失败条目的结果可能需要核对；请刷新并核对目标数据后继续，勿重放整批更改：{detail}")]
    PartialWrite {
        phase: Option<DisconnectPhase>,
        applied: usize,
        failed_index: usize,
        detail: String,
    },
}

impl AppError {
    pub fn msg(text: impl Into<String>) -> Self {
        AppError::Message(text.into())
    }

    pub fn driver(detail: impl std::fmt::Display, disconnected: bool) -> Self {
        if disconnected {
            Self::ConnectionLost {
                phase: DisconnectPhase::OutcomeUnknown,
                detail: detail.to_string(),
            }
        } else {
            Self::msg(detail.to_string())
        }
    }

    pub fn context(self, description: &str) -> Self {
        match self {
            Self::Message(detail) => Self::msg(format!("{description}：{detail}")),
            Self::ConnectionLost { phase, detail } => Self::ConnectionLost {
                phase,
                detail: format!("{description}：{detail}"),
            },
            other => other,
        }
    }

    pub fn before_send(self) -> Self {
        match self {
            Self::ConnectionLost { detail, .. } => Self::ConnectionLost {
                phase: DisconnectPhase::BeforeSend,
                detail,
            },
            other => other,
        }
    }

    pub fn disconnect_phase(&self) -> Option<DisconnectPhase> {
        match self {
            Self::ConnectionLost { phase, .. } => Some(*phase),
            Self::PartialWrite { phase, .. } => *phase,
            Self::SessionExpired => Some(DisconnectPhase::BeforeSend),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::SessionExpired => "sessionExpired",
            Self::ConnectionLost { .. } => "connectionLost",
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
            disconnect_phase: self.disconnect_phase(),
            message: self.to_string(),
            committed: None,
            batches: None,
            applied: None,
            failed_index: None,
        };
        match self {
            Self::Message(_) => {}
            Self::SessionExpired => payload.kind = ErrorKind::SessionExpired,
            Self::ConnectionLost { .. } => payload.kind = ErrorKind::ConnectionLost,
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
