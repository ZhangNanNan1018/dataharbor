use super::storage::{store_lock, write_json_unlocked};
use super::types::{AiAskRequest, AiSettingsPublic, AiSettingsRequest};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::fs;

const DEFAULT_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

fn endpoint_url(endpoint: &str) -> AppResult<tauri::Url> {
    let url = tauri::Url::parse(if endpoint.is_empty() {
        DEFAULT_ENDPOINT
    } else {
        endpoint
    })
    .map_err(|_| AppError::msg("AI 接口地址无效"))?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        return Err(AppError::msg(
            "AI 接口必须使用 HTTPS；只有本机 localhost/回环地址允许 HTTP，地址中不能包含账号或密码",
        ));
    }
    Ok(url)
}

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
            DEFAULT_ENDPOINT.into()
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
    let guard = store_lock(state, "ai.json")?;
    let mut file = read_ai(state)?;
    let next_endpoint = if request.endpoint.trim().is_empty() {
        file.endpoint.as_str()
    } else {
        request.endpoint.trim()
    };
    let next_origin = endpoint_url(next_endpoint)?.origin();
    let new_key = request
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let same_origin = endpoint_url(&file.endpoint).is_ok_and(|url| url.origin() == next_origin);
    if !file.key_enc.is_empty() && !same_origin && new_key.is_none() && !request.clear_key {
        return Err(AppError::msg(
            "AI 服务地址已跨域变更，请重新填写该服务的 API 密钥或明确清除旧密钥；旧设置保持不变",
        ));
    }
    if !request.endpoint.trim().is_empty() {
        file.endpoint = request.endpoint.trim().to_string();
    }
    if !request.model.trim().is_empty() {
        file.model = request.model.trim().to_string();
    }
    if request.clear_key {
        file.key_enc.clear();
    } else if let Some(key) = new_key {
        file.key_enc = state.encrypt_secret(key)?;
    }
    write_json_unlocked(state, "ai.json", &file)?;
    drop(guard);
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
    let endpoint = endpoint_url(&file.endpoint)?;
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
    let response = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .post(endpoint.as_str())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_change_requires_replacement_key_and_preserves_old_settings_on_error() {
        let dir =
            std::env::temp_dir().join(format!("dataharbor-ai-origin-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        {
            let state = AppState::open_in(dir.clone(), true).unwrap();
            let request = |endpoint: &str, key: Option<&str>| AiSettingsRequest {
                endpoint: endpoint.into(),
                model: "test".into(),
                api_key: key.map(str::to_string),
                clear_key: false,
            };
            save_ai_settings(
                &state,
                request("https://one.example/v1/chat", Some("key-one")),
            )
            .unwrap();
            let before = fs::read(dir.join("ai.json")).unwrap();
            assert!(
                save_ai_settings(&state, request("https://two.example/v1/chat", None)).is_err()
            );
            assert_eq!(fs::read(dir.join("ai.json")).unwrap(), before);
            save_ai_settings(&state, request("https://one.example:443/v2/chat", None)).unwrap();
            assert_eq!(
                state
                    .decrypt_secret(&read_ai(&state).unwrap().key_enc)
                    .unwrap(),
                "key-one"
            );
            save_ai_settings(
                &state,
                request("https://two.example/v1/chat", Some("key-two")),
            )
            .unwrap();
            assert_eq!(
                state
                    .decrypt_secret(&read_ai(&state).unwrap().key_enc)
                    .unwrap(),
                "key-two"
            );
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ai_transport_allows_local_http_but_rejects_remote_http_and_url_credentials() {
        for endpoint in [
            "http://localhost:11434/v1/chat",
            "http://127.0.0.1:8080/chat",
            "http://[::1]:8080/chat",
            "https://service.example/chat",
        ] {
            assert!(endpoint_url(endpoint).is_ok(), "{endpoint}");
        }
        for endpoint in [
            "http://remote.example/chat",
            "http://localhost.attacker.example/chat",
            "https://key@service.example/chat",
            "file:///secrets",
        ] {
            assert!(endpoint_url(endpoint).is_err(), "{endpoint}");
        }
    }
}
