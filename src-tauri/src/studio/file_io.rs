use super::storage::{cell_string, escape_xml};
use super::table_data::{load_all_operation, CEILING};
use super::types::{FileExportRequest, FileExportResult, FileImportRequest};
use crate::data::{commit_changes_with_operation, delimited};
use crate::error::{AppError, AppResult};
use crate::models::{CommitRequest, FieldValue, RowChange};
use crate::operations::{AtomicOutput, OperationContext};
use crate::state::AppState;
use serde_json::{json, Value as JsonValue};
use std::fs;
use std::path::Path;

pub fn export_file(state: &AppState, request: FileExportRequest) -> AppResult<FileExportResult> {
    export_file_with_operation(state, request, &OperationContext::detached())
}

pub fn export_file_with_operation(
    state: &AppState,
    request: FileExportRequest,
    operation: &OperationContext,
) -> AppResult<FileExportResult> {
    operation.checkpoint()?;
    let format = request.format.to_ascii_lowercase();
    let (columns, rows, total, hit_ceiling) = load_all_operation(
        state,
        &request.connection_id,
        &request.database,
        &request.schema,
        &request.table,
        operation,
    )?;
    let text_columns: Vec<String> = columns.iter().map(|column| column.name.clone()).collect();
    operation.checkpoint()?;
    operation.report(
        "写出文件",
        0,
        Some(rows.len() as u64),
        "取消在当前格式处理完成后停止，不会替换旧输出",
    );
    let output = AtomicOutput::new(Path::new(&request.path))?;
    let output_path = output
        .path()
        .to_str()
        .ok_or_else(|| AppError::msg("输出路径无效"))?;
    match format.as_str() {
        "json" => {
            let objects: Vec<JsonValue> = rows
                .iter()
                .map(|row| row_object(&text_columns, row))
                .collect();
            fs::write(
                output_path,
                serde_json::to_string_pretty(&objects).unwrap_or_else(|_| "[]".into()),
            )
            .map_err(|err| AppError::msg(err.to_string()))?;
        }
        "xml" => {
            fs::write(output_path, rows_to_xml(&text_columns, &rows))
                .map_err(|err| AppError::msg(err.to_string()))?;
        }
        "txt" => {
            fs::write(output_path, rows_to_delimited(&text_columns, &rows, '\t')?)
                .map_err(|err| AppError::msg(err.to_string()))?;
        }
        "csv" => {
            fs::write(output_path, rows_to_delimited(&text_columns, &rows, ',')?)
                .map_err(|err| AppError::msg(err.to_string()))?;
        }
        "xlsx" => write_xlsx(output_path, &text_columns, &rows)?,
        "dbf" => crate::dbf::write_dbf(Path::new(output_path), &text_columns, &rows)?,
        "parquet" => crate::parquetio::write_table(Path::new(output_path), &text_columns, &rows)?,
        "pdf" => {
            let mut lines = vec![text_columns.join(" | ")];
            for row in &rows {
                lines.push(
                    row.iter()
                        .map(|value| cell_string(Some(value)))
                        .collect::<Vec<_>>()
                        .join(" | "),
                );
            }
            crate::pdf::write_text_pdf(Path::new(output_path), &request.table, &lines)?;
        }
        other => return Err(AppError::msg(format!("不支持的导出格式 {other}"))),
    }
    operation.checkpoint()?;
    output.commit()?;
    operation.report(
        "文件已保存",
        rows.len() as u64,
        Some(rows.len() as u64),
        &request.path,
    );
    Ok(FileExportResult {
        path: request.path,
        row_count: rows.len() as u64,
        truncated: hit_ceiling || total > rows.len() as u64,
        kind: "table-export".into(),
        note: if hit_ceiling || total > rows.len() as u64 {
            format!("导出已截断：共 {total} 行，仅导出 {} 行", rows.len())
        } else {
            String::new()
        },
    })
}

#[cfg(test)]
pub fn import_file(state: &AppState, request: FileImportRequest) -> AppResult<usize> {
    import_file_with_operation(state, request, &OperationContext::detached())
}

pub fn import_file_with_operation(
    state: &AppState,
    request: FileImportRequest,
    operation: &OperationContext,
) -> AppResult<usize> {
    operation.checkpoint()?;
    operation.report(
        "解析文件",
        0,
        None,
        "尚未写入数据库；复杂格式解析将在当前步骤结束后响应取消",
    );
    let format = request.format.to_ascii_lowercase();
    let (headers, records) = match format.as_str() {
        "json" => read_json_table(&request.path)?,
        "xml" => read_xml_table(&request.path)?,
        "txt" => read_delimited_operation(&request.path, '\t', operation)?,
        "csv" => read_delimited_operation(&request.path, ',', operation)?,
        "xlsx" => read_xlsx_table(&request.path)?,
        "dbf" => crate::dbf::read_dbf(Path::new(&request.path))?,
        "parquet" => crate::parquetio::read_table(Path::new(&request.path))?,
        "mdb" | "accdb" | "access" => read_access_table(&request.path, &request.table)?,
        "odbc" => read_odbc_table(&request.path, &request.table)?,
        other => return Err(AppError::msg(format!("不支持的导入格式 {other}"))),
    };
    operation.checkpoint()?;
    if records.len() > CEILING as usize {
        return Err(AppError::msg(format!("一次最多导入 {CEILING} 行")));
    }
    let changes = records
        .into_iter()
        .map(|row| {
            operation.checkpoint()?;
            Ok(RowChange {
                kind: "insert".into(),
                original: None,
                pk: Vec::new(),
                values: headers
                    .iter()
                    .cloned()
                    .zip(row)
                    .map(|(column, value)| FieldValue { column, value })
                    .collect(),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    if changes.is_empty() {
        return Err(AppError::msg("文件里没有数据行"));
    }
    let result = commit_changes_with_operation(
        state,
        CommitRequest {
            connection_id: request.connection_id,
            database: request.database,
            schema: request.schema,
            table: request.table,
            changes,
        },
        operation,
    )?;
    Ok(result.applied)
}

pub fn export_pdf_text(title: &str, body: &str, path: &str) -> AppResult<String> {
    let lines: Vec<String> = body.lines().map(|line| line.to_string()).collect();
    crate::pdf::write_text_pdf(Path::new(path), title, &lines)?;
    Ok(path.to_string())
}

fn read_access_table(path: &str, table: &str) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    if crate::native::tool_path("mdb-export").is_none() {
        return Err(AppError::msg(
            "没有找到 mdb-export。Linux 可安装 mdbtools。Windows 上的 Access 请用 ODBC（本机 isql 或微软 ACE 驱动），当前构建不链接 Access 引擎。",
        ));
    }
    let output = std::process::Command::new("mdb-export")
        .args(["-d,", path, table])
        .output()
        .map_err(|err| AppError::msg(err.to_string()))?;
    if !output.status.success() {
        return Err(AppError::msg(format!(
            "mdb-export 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    parse_delimited_text(&text, ',')
}

fn read_odbc_table(dsn: &str, table: &str) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    if crate::native::tool_path("isql").is_none() {
        return Err(AppError::msg(
            "没有找到 unixODBC 的 isql。ODBC 导入把路径栏当作 DSN 名称，表名作为要读取的表。Windows 上请安装 ODBC 驱动后再试。本程序不内嵌 ODBC 管理器。",
        ));
    }
    let sql = format!("SELECT * FROM {table}");
    let output = std::process::Command::new("isql")
        .args(["-b", "-d,", dsn, &sql])
        .output()
        .map_err(|err| AppError::msg(err.to_string()))?;
    if !output.status.success() {
        return Err(AppError::msg(format!(
            "isql 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    parse_delimited_text(&String::from_utf8_lossy(&output.stdout), ',')
}

fn parse_delimited_text(text: &str, sep: char) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    delimited::read(text.as_bytes(), sep as u8, CEILING as usize)
}

fn row_object(columns: &[String], row: &[JsonValue]) -> JsonValue {
    let mut map = serde_json::Map::new();
    for (index, name) in columns.iter().enumerate() {
        map.insert(
            name.clone(),
            row.get(index).cloned().unwrap_or(JsonValue::Null),
        );
    }
    JsonValue::Object(map)
}

fn rows_to_delimited(columns: &[String], rows: &[Vec<JsonValue>], sep: char) -> AppResult<Vec<u8>> {
    let mut output = Vec::new();
    delimited::write(&mut output, columns, rows, sep as u8)?;
    Ok(output)
}

fn rows_to_xml(columns: &[String], rows: &[Vec<JsonValue>]) -> String {
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rows>\n");
    for row in rows {
        text.push_str("  <row>\n");
        for (index, name) in columns.iter().enumerate() {
            let value = cell_string(row.get(index));
            text.push_str(&format!(
                "    <col name=\"{}\">{}</col>\n",
                escape_xml(name),
                escape_xml(&value)
            ));
        }
        text.push_str("  </row>\n");
    }
    text.push_str("</rows>\n");
    text
}

fn read_delimited_operation(
    path: &str,
    sep: char,
    operation: &OperationContext,
) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    let file = fs::File::open(path).map_err(|err| AppError::msg(err.to_string()))?;
    delimited::read_with_operation(file, sep as u8, CEILING as usize, operation)
}

fn parse_cell(text: String) -> JsonValue {
    JsonValue::String(text)
}

fn read_json_table(path: &str) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    let text = fs::read_to_string(path).map_err(|err| AppError::msg(err.to_string()))?;
    let value: JsonValue =
        serde_json::from_str(&text).map_err(|err| AppError::msg(err.to_string()))?;
    let array = value
        .as_array()
        .ok_or_else(|| AppError::msg("JSON 需要是对象数组"))?;
    let mut headers = Vec::new();
    for item in array {
        if let Some(map) = item.as_object() {
            for key in map.keys() {
                if !headers.iter().any(|name| name == key) {
                    headers.push(key.clone());
                }
            }
        }
    }
    let rows = array
        .iter()
        .map(|item| {
            headers
                .iter()
                .map(|name| item.get(name).cloned().unwrap_or(JsonValue::Null))
                .collect()
        })
        .collect();
    Ok((headers, rows))
}

fn read_xml_table(path: &str) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    let text = fs::read_to_string(path).map_err(|err| AppError::msg(err.to_string()))?;
    let mut headers = Vec::new();
    let mut rows = Vec::new();
    let mut current: Vec<(String, String)> = Vec::new();
    let mut in_col = false;
    let mut name = String::new();
    let mut value = String::new();
    for raw in text.split('<').skip(1) {
        let (tag, rest) = raw.split_once('>').unwrap_or((raw, ""));
        let closed = tag.ends_with('/');
        let tag_name = tag
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if tag.starts_with('/') {
            if tag_name == "col" {
                if !headers.iter().any(|item| item == &name) {
                    headers.push(name.clone());
                }
                current.push((name.clone(), value.clone()));
                in_col = false;
            } else if tag_name == "row" && !current.is_empty() {
                rows.push(std::mem::take(&mut current));
            }
            continue;
        }
        if tag_name == "row" {
            current.clear();
        } else if tag_name == "col" && !closed {
            name = attr_value(tag, "name");
            value = rest.to_string();
            in_col = true;
        } else if in_col {
            value.push('<');
            value.push_str(raw);
        }
    }
    let records = rows
        .into_iter()
        .map(|pairs| {
            headers
                .iter()
                .map(|header| {
                    pairs
                        .iter()
                        .find(|(key, _)| key == header)
                        .map(|(_, text)| unescape_xml(text))
                        .map(parse_cell)
                        .unwrap_or(JsonValue::Null)
                })
                .collect()
        })
        .collect();
    Ok((headers, records))
}

fn attr_value(tag: &str, key: &str) -> String {
    let pattern = format!("{key}=\"");
    tag.split_once(&pattern)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| unescape_xml(value))
        .unwrap_or_default()
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

fn read_xlsx_table(path: &str) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    use calamine::{open_workbook_auto, Reader};
    let mut book = open_workbook_auto(path).map_err(|err| AppError::msg(err.to_string()))?;
    let range = book
        .worksheet_range_at(0)
        .ok_or_else(|| AppError::msg("工作簿是空的"))?
        .map_err(|err| AppError::msg(err.to_string()))?;
    let mut lines = range.rows();
    let headers = lines
        .next()
        .ok_or_else(|| AppError::msg("工作表没有表头"))?
        .iter()
        .map(cell_data)
        .map(|value| cell_string(Some(&value)))
        .collect::<Vec<_>>();
    let rows = lines
        .map(|row| row.iter().map(cell_data).collect())
        .collect();
    Ok((headers, rows))
}

fn cell_data(cell: &calamine::Data) -> JsonValue {
    match cell {
        calamine::Data::Int(value) => json!(value),
        calamine::Data::Float(value) => json!(value),
        calamine::Data::String(text) => JsonValue::String(text.clone()),
        calamine::Data::Bool(flag) => JsonValue::Bool(*flag),
        calamine::Data::DateTimeIso(text) | calamine::Data::DurationIso(text) => {
            JsonValue::String(text.clone())
        }
        calamine::Data::Empty => JsonValue::Null,
        other => JsonValue::String(format!("{other:?}")),
    }
}

fn write_xlsx(path: &str, columns: &[String], rows: &[Vec<JsonValue>]) -> AppResult<()> {
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    for (index, name) in columns.iter().enumerate() {
        sheet
            .write_string(0, index as u16, name)
            .map_err(|err| AppError::msg(err.to_string()))?;
    }
    for (row_index, row) in rows.iter().enumerate() {
        for (col_index, value) in row.iter().enumerate() {
            sheet
                .write_string(
                    row_index as u32 + 1,
                    col_index as u16,
                    cell_string(Some(value)),
                )
                .map_err(|err| AppError::msg(err.to_string()))?;
        }
    }
    book.save(path)
        .map_err(|err| AppError::msg(err.to_string()))
}
