use super::storage::write_json;
use super::types::{AiAskRequest, AiSettingsPublic, AiSettingsRequest};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::fs;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiFile {
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    key_enc: String,
}

pub fn ai_settings(state: &AppState) -> AppResult<AiSettingsPublic> {
    let file = read_ai(state)?;
    Ok(AiSettingsPublic {
        endpoint: if file.endpoint.is_empty() {
            "https://api.openai.com/v1/chat/completions".into()
        } else {
            file.endpoint
        },
        model: if file.model.is_empty() {
            "gpt-4o-mini".into()
        } else {
            file.model
        },
        has_key: !file.key_enc.is_empty(),
    })
}

pub fn save_ai_settings(
    state: &AppState,
    request: AiSettingsRequest,
) -> AppResult<AiSettingsPublic> {
    let mut file = read_ai(state)?;
    if !request.endpoint.trim().is_empty() {
        file.endpoint = request.endpoint.trim().to_string();
    }
    if !request.model.trim().is_empty() {
        file.model = request.model.trim().to_string();
    }
    if request.clear_key {
        file.key_enc.clear();
    } else if let Some(key) = request
        .api_key
        .as_ref()
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
    {
        file.key_enc = state.encrypt_secret(key)?;
    }
    write_json(state, "ai.json", &file)?;
    ai_settings(state)
}

pub fn ai_ask(state: &AppState, request: AiAskRequest) -> AppResult<String> {
    let file = read_ai(state)?;
    if file.key_enc.is_empty() {
        return Err(AppError::msg(
            "请先在 AI 助手里保存自己的 API 密钥。DataHarbor 不会内置密钥，也不会编造回复。",
        ));
    }
    let key = state.decrypt_secret(&file.key_enc)?;
    let endpoint = if file.endpoint.is_empty() {
        "https://api.openai.com/v1/chat/completions".to_string()
    } else {
        file.endpoint.clone()
    };
    let model = if !request.model.trim().is_empty() {
        request.model.trim().to_string()
    } else if file.model.is_empty() {
        "gpt-4o-mini".to_string()
    } else {
        file.model.clone()
    };
    let system = "你是 DataHarbor 的 SQL 助手。只根据用户给出的语句和结构说明回答。不要声称已经连接数据库。用简体中文。";
    let user = match request.task.as_str() {
        "optimize" => format!("优化这段 {} SQL，说明理由并给出改写：\n{}\n结构：\n{}", request.engine, request.sql, request.schema_text),
        "format" => format!("只输出美化后的 SQL：\n{}", request.sql),
        "convert" => format!("把下面的 {} SQL 转成 {}，只输出 SQL：\n{}", request.engine, request.target_engine, request.sql),
        "fix" => format!("修复这段 {} SQL。先给修正语句，再简短说明：\n{}\n{}", request.engine, request.sql, request.prompt),
        "chat" => format!("问题：{}\nSQL：{}\n结构：\n{}", request.prompt, request.sql, request.schema_text),
        "fill" => format!(
            "根据列说明为这一行补上合理的示例值。只输出一个 JSON 对象，键是列名，不要解释。\n列：{}\n已有值：{}\n要求：{}",
            request.schema_text, request.sql, request.prompt
        ),
        _ => format!("解释这段 {} SQL 在做什么：\n{}\n结构：\n{}", request.engine, request.sql, request.schema_text),
    };
    let payload = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]
    });
    let body_text =
        serde_json::to_string(&payload).map_err(|err| AppError::msg(err.to_string()))?;
    let response = ureq::post(&endpoint)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .send_string(&body_text)
        .map_err(|err| AppError::msg(format!("AI 接口失败: {err}")))?;
    let raw = response
        .into_string()
        .map_err(|err| AppError::msg(err.to_string()))?;
    let body: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::String(raw));
    body.pointer("/choices/0/message/content")
        .and_then(|value| value.as_str())
        .map(|text| text.to_string())
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| AppError::msg(format!("接口没有返回文本：{}", body)))
}

fn read_ai(state: &AppState) -> AppResult<AiFile> {
    let path = state.config_dir().join("ai.json");
    if !path.exists() {
        return Ok(AiFile {
            endpoint: String::new(),
            model: String::new(),
            key_enc: String::new(),
        });
    }
    let text = fs::read_to_string(path).map_err(|err| AppError::msg(err.to_string()))?;
    serde_json::from_str(&text).map_err(|err| AppError::msg(err.to_string()))
}
