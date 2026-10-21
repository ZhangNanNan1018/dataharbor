use super::catalog::er_diagram;
use super::query_tools::preview_design;
use super::storage::{escape_xml, read_vec, update_vec};
use super::types::{
    Dashboard, DataModel, DesignColumn, DesignRequest, ModelAttr, ModelEntity, ModelRel, ObjectRef,
};
use crate::error::{AppError, AppResult};
use crate::models::Engine;
use crate::state::AppState;
use uuid::Uuid;

pub fn list_dashboards(state: &AppState) -> AppResult<Vec<Dashboard>> {
    read_vec(state, "dashboards.json")
}

pub fn save_dashboard(state: &AppState, mut board: Dashboard) -> AppResult<Dashboard> {
    if board.id.is_empty() {
        board.id = Uuid::new_v4().to_string();
    }
    if board.name.trim().is_empty() {
        board.name = "未命名看板".into();
    }
    update_vec(state, "dashboards.json", |items: &mut Vec<Dashboard>| {
        if let Some(slot) = items.iter_mut().find(|item| item.id == board.id) {
            *slot = board.clone();
        } else {
            items.push(board.clone());
        }
        Ok(board)
    })
}

pub fn delete_dashboard(state: &AppState, id: &str) -> AppResult<()> {
    update_vec(state, "dashboards.json", |items: &mut Vec<Dashboard>| {
        items.retain(|item| item.id != id);
        Ok(())
    })
}

pub fn list_models(state: &AppState) -> AppResult<Vec<DataModel>> {
    read_vec(state, "models.json")
}

pub fn save_model(state: &AppState, mut model: DataModel) -> AppResult<DataModel> {
    if model.id.is_empty() {
        model.id = Uuid::new_v4().to_string();
    }
    if model.name.trim().is_empty() {
        return Err(AppError::msg("模型需要名称"));
    }
    update_vec(state, "models.json", |items: &mut Vec<DataModel>| {
        if let Some(slot) = items.iter_mut().find(|item| item.id == model.id) {
            *slot = model.clone();
        } else {
            items.push(model.clone());
        }
        Ok(model)
    })
}

pub fn delete_model(state: &AppState, id: &str) -> AppResult<()> {
    update_vec(state, "models.json", |items: &mut Vec<DataModel>| {
        items.retain(|item| item.id != id);
        Ok(())
    })
}

pub fn reverse_model(state: &AppState, request: &ObjectRef, name: &str) -> AppResult<DataModel> {
    let graph = er_diagram(state, request)?;
    let model = DataModel {
        id: Uuid::new_v4().to_string(),
        name: if name.trim().is_empty() {
            "反向模型".into()
        } else {
            name.to_string()
        },
        level: "physical".into(),
        note: "从当前库反向读取。概念和逻辑层可以在此基础上改名。".into(),
        entities: graph
            .tables
            .into_iter()
            .map(|table| ModelEntity {
                name: table.name,
                attributes: table
                    .columns
                    .into_iter()
                    .map(|column| ModelAttr {
                        name: column.name,
                        data_type: column.data_type,
                        pk: column.primary_key,
                        nullable: column.nullable,
                    })
                    .collect(),
            })
            .collect(),
        relations: graph
            .edges
            .into_iter()
            .map(|edge| ModelRel {
                from_entity: edge.from_table,
                to_entity: edge.to_table,
                from_attr: edge.from_column,
                to_attr: edge.to_column,
                label: "fk".into(),
            })
            .collect(),
    };
    save_model(state, model)
}

pub fn forward_sql(model: &DataModel, engine: Engine) -> AppResult<String> {
    let mut sql = format!("-- DataHarbor {} 模型正向工程\n\n", model.level);
    for entity in &model.entities {
        let request = DesignRequest {
            engine,
            database: String::new(),
            schema: if engine == Engine::Postgres {
                "public".into()
            } else {
                String::new()
            },
            name: entity.name.clone(),
            kind: "table".into(),
            columns: entity
                .attributes
                .iter()
                .map(|attr| DesignColumn {
                    name: attr.name.clone(),
                    data_type: if attr.data_type.is_empty() {
                        "TEXT".into()
                    } else {
                        attr.data_type.clone()
                    },
                    nullable: attr.nullable,
                    primary_key: attr.pk,
                })
                .collect(),
            body: String::new(),
        };
        sql.push_str(&preview_design(&request)?);
        sql.push_str("\n\n");
    }
    Ok(sql)
}

pub fn model_html(model: &DataModel) -> String {
    let mut html = format!(
        "<!DOCTYPE html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1><p>{} · {}</p>",
        escape_xml(&model.name),
        escape_xml(&model.name),
        escape_xml(&model.level),
        escape_xml(&model.note)
    );
    for entity in &model.entities {
        html.push_str(&format!("<h2>{}</h2><table border=\"1\" cellpadding=\"4\"><tr><th>属性</th><th>类型</th><th>主键</th></tr>", escape_xml(&entity.name)));
        for attr in &entity.attributes {
            html.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_xml(&attr.name),
                escape_xml(&attr.data_type),
                if attr.pk { "是" } else { "" }
            ));
        }
        html.push_str("</table>");
    }
    html.push_str(
        "<p>用浏览器打印即可保存 PDF。未内置中文字体，因此不直接生成 PDF。</p></body></html>",
    );
    html
}
