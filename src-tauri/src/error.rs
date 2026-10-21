use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

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
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Message(_) => serializer.serialize_str(&self.to_string()),
            Self::OperationMissing => {
                use serde::ser::SerializeStruct;
                let mut value = serializer.serialize_struct("OperationMissing", 2)?;
                value.serialize_field("kind", "operationMissing")?;
                value.serialize_field("message", &self.to_string())?;
                value.end()
            }
            Self::Cancelled { committed, batches } => {
                use serde::ser::SerializeStruct;
                let mut value = serializer.serialize_struct("Cancelled", 4)?;
                value.serialize_field("kind", "cancelled")?;
                value.serialize_field("message", &self.to_string())?;
                value.serialize_field("committed", committed)?;
                value.serialize_field("batches", batches)?;
                value.end()
            }
            Self::PartialWrite {
                applied,
                failed_index,
                ..
            } => {
                use serde::ser::SerializeStruct;
                let mut value = serializer.serialize_struct("PartialWrite", 4)?;
                value.serialize_field("kind", "partialWrite")?;
                value.serialize_field("message", &self.to_string())?;
                value.serialize_field("applied", applied)?;
                value.serialize_field("failedIndex", failed_index)?;
                value.end()
            }
        }
    }
}
