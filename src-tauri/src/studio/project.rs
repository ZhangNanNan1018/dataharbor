use super::automation::{list_jobs, update_jobs};
use super::storage::{now_secs, read_vec, update_vec};
use super::types::{Dashboard, DataModel, Favorite, Job, Snippet};
use crate::error::{AppError, AppResult};
use crate::models::StoredProfile;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use uuid::Uuid;

pub fn list_snippets(state: &AppState) -> AppResult<Vec<Snippet>> {
    read_vec(state, "snippets.json")
}

pub fn save_snippet(state: &AppState, mut snippet: Snippet) -> AppResult<Snippet> {
    if snippet.name.trim().is_empty() {
        return Err(AppError::msg("片段需要名称"));
    }
    if snippet.id.is_empty() {
        snippet.id = Uuid::new_v4().to_string();
    }
    snippet.updated_at = now_secs().to_string();
    update_vec(state, "snippets.json", |items: &mut Vec<Snippet>| {
        if let Some(slot) = items.iter_mut().find(|item| item.id == snippet.id) {
            *slot = snippet.clone();
        } else {
            items.push(snippet.clone());
        }
        Ok(snippet)
    })
}

pub fn delete_snippet(state: &AppState, id: &str) -> AppResult<()> {
    update_vec(state, "snippets.json", |items: &mut Vec<Snippet>| {
        items.retain(|item| item.id != id);
        Ok(())
    })
}

pub fn list_favorites(state: &AppState) -> AppResult<Vec<Favorite>> {
    read_vec(state, "favorites.json")
}

pub fn save_favorite(state: &AppState, mut favorite: Favorite) -> AppResult<Favorite> {
    if favorite.id.is_empty() {
        favorite.id = Uuid::new_v4().to_string();
    }
    update_vec(state, "favorites.json", |items: &mut Vec<Favorite>| {
        if let Some(existing) = items.iter().find(|item| {
            item.connection_id == favorite.connection_id
                && item.object_name == favorite.object_name
                && item.database == favorite.database
        }) {
            return Ok(existing.clone());
        }
        items.push(favorite.clone());
        Ok(favorite)
    })
}

pub fn delete_favorite(state: &AppState, id: &str) -> AppResult<()> {
    update_vec(state, "favorites.json", |items: &mut Vec<Favorite>| {
        items.retain(|item| item.id != id);
        Ok(())
    })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFile {
    version: u32,
    profiles: Vec<StoredProfile>,
    snippets: Vec<Snippet>,
    jobs: Vec<Job>,
    dashboards: Vec<Dashboard>,
    models: Vec<DataModel>,
    favorites: Vec<Favorite>,
}

pub fn export_project(state: &AppState, path: &str) -> AppResult<String> {
    let file = ProjectFile {
        version: 1,
        profiles: state.profiles_snapshot(),
        snippets: read_vec(state, "snippets.json")?,
        jobs: list_jobs(state)?,
        dashboards: read_vec(state, "dashboards.json")?,
        models: read_vec(state, "models.json")?,
        favorites: read_vec(state, "favorites.json")?,
    };
    fs::write(
        path,
        serde_json::to_string_pretty(&file).map_err(|err| AppError::msg(err.to_string()))?,
    )
    .map_err(|err| AppError::msg(err.to_string()))?;
    Ok(path.to_string())
}

pub fn import_project(state: &AppState, path: &str) -> AppResult<String> {
    let text = fs::read_to_string(path).map_err(|err| AppError::msg(err.to_string()))?;
    let file: ProjectFile = serde_json::from_str(&text)
        .map_err(|err| AppError::msg(format!("项目包无法读取: {err}")))?;
    let (added, needs_credentials) = state.import_profiles(file.profiles)?;
    merge_vec::<Snippet>(state, "snippets.json", file.snippets)?;
    let (imported_jobs, skipped_jobs) = import_jobs_disabled(state, file.jobs)?;
    merge_vec::<Dashboard>(state, "dashboards.json", file.dashboards)?;
    merge_vec::<DataModel>(state, "models.json", file.models)?;
    merge_vec::<Favorite>(state, "favorites.json", file.favorites)?;
    Ok(format!("已并入 {added} 个新连接，以及查询片段、看板、模型和收藏。其中 {needs_credentials} 个连接的加密凭据无法使用本机密钥解密，已仅导入连接资料，需要重新填写密码（原项目包未修改）。新增任务 {imported_jobs} 个，均已禁用，需检查 SQL、连接和输出路径后手动启用；跳过同 ID 任务 {skipped_jobs} 个，已有本地任务保持原样。"))
}

fn import_jobs_disabled(state: &AppState, extra: Vec<Job>) -> AppResult<(usize, usize)> {
    update_jobs(state, |jobs| {
        let mut added = 0;
        let mut skipped = 0;
        for mut job in extra {
            if jobs.iter().any(|existing| existing.id == job.id) {
                skipped += 1;
                continue;
            }
            job.enabled = false;
            job.revision = Uuid::new_v4().to_string();
            job.next_run_at = 0;
            job.last_message =
                "从项目包导入，默认禁用；请检查 SQL、连接和输出路径后手动启用。".into();
            jobs.push(job);
            added += 1;
        }
        Ok((added, skipped))
    })
}

fn merge_vec<T>(state: &AppState, name: &str, extra: Vec<T>) -> AppResult<()>
where
    T: Serialize + serde::de::DeserializeOwned + HasId,
{
    update_vec(state, name, |items: &mut Vec<T>| {
        for item in extra {
            if items
                .iter()
                .any(|existing| existing.id_of() == item.id_of())
            {
                continue;
            }
            items.push(item);
        }
        Ok(())
    })
}

trait HasId {
    fn id_of(&self) -> &str;
}
impl HasId for Snippet {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for Job {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for Dashboard {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for DataModel {
    fn id_of(&self) -> &str {
        &self.id
    }
}
impl HasId for Favorite {
    fn id_of(&self) -> &str {
        &self.id
    }
}
