use super::read_batches::{ReadSession, TableReader};
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
use std::io::{BufWriter, Write};
use std::path::Path;

#[cfg(test)]
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
    if matches!(format.as_str(), "csv" | "txt" | "json" | "xml") {
        return export_streaming_file(state, request, operation);
    }
    if !matches!(format.as_str(), "xlsx" | "dbf" | "parquet" | "pdf") {
        return Err(AppError::msg(format!("不支持的导出格式 {format}")));
    }
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

fn export_streaming_file(
    state: &AppState,
    request: FileExportRequest,
    operation: &OperationContext,
) -> AppResult<FileExportResult> {
    export_streaming_file_with_hook(state, request, operation, &mut |_| {})
}

pub(super) fn export_streaming_file_with_hook(
    state: &AppState,
    request: FileExportRequest,
    operation: &OperationContext,
    after_batch: &mut dyn FnMut(u64),
) -> AppResult<FileExportResult> {
    let mut read = ReadSession::open(state, &request.connection_id, &request.database, operation)?;
    let mut reader = TableReader::new(
        &mut *read.session,
        read.snapshot,
        &request.database,
        &request.schema,
        &request.table,
        None,
        operation,
    )?;
    // Document drivers discover the complete display schema in their single
    // bounded result. SQL readers already have schema before the first batch.
    let mut batch = reader.next(operation)?;
    let columns = reader
        .columns
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>();
    let output = AtomicOutput::new(Path::new(&request.path))?;
    let file = fs::File::create(output.path()).map_err(|e| AppError::msg(e.to_string()))?;
    let mut writer = TextTableWriter::new(
        BufWriter::new(file),
        &request.format.to_ascii_lowercase(),
        &columns,
    )?;
    while let Some(rows) = batch {
        writer.batch(&columns, &rows, operation)?;
        after_batch(reader.loaded);
        batch = reader.next(operation)?;
    }
    writer.finish()?;
    let (row_count, total, truncated) = (reader.loaded, reader.total, reader.truncated());
    drop(reader);
    read.finish()?;
    operation.checkpoint()?;
    output.commit()?;
    operation.report("文件已保存", row_count, Some(row_count), &request.path);
    Ok(FileExportResult {
        path: request.path,
        row_count,
        truncated,
        kind: "table-export".into(),
        note: if truncated {
            format!(
                "导出已截断：共 {total} 行，仅导出 {row_count} 行；当前表无法使用安全快照主键扫描"
            )
        } else {
            String::new()
        },
    })
}

enum TextTableWriter<W: Write> {
    Delimited(csv::Writer<W>),
    Json(W, bool),
    Xml(W),
}

impl<W: Write> TextTableWriter<W> {
    fn new(mut writer: W, format: &str, columns: &[String]) -> AppResult<Self> {
        match format {
            "csv" | "txt" => {
                let mut csv = csv::WriterBuilder::new()
                    .delimiter(if format == "txt" { b'\t' } else { b',' })
                    .from_writer(writer);
                csv.write_record(columns).map_err(io_error)?;
                Ok(Self::Delimited(csv))
            }
            "json" => {
                writer.write_all(b"[\n").map_err(io_error)?;
                Ok(Self::Json(writer, true))
            }
            "xml" => {
                writer
                    .write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rows>\n")
                    .map_err(io_error)?;
                Ok(Self::Xml(writer))
            }
            _ => Err(AppError::msg("不支持的流式导出格式")),
        }
    }

    fn batch(
        &mut self,
        columns: &[String],
        rows: &[Vec<JsonValue>],
        operation: &OperationContext,
    ) -> AppResult<()> {
        for row in rows {
            operation.checkpoint()?;
            match self {
                Self::Delimited(writer) => writer
                    .write_record(row.iter().map(delimited::cell))
                    .map_err(io_error)?,
                Self::Json(writer, first) => {
                    if !*first {
                        writer.write_all(b",\n").map_err(io_error)?;
                    }
                    serde_json::to_writer(&mut *writer, &row_object(columns, row))
                        .map_err(io_error)?;
                    *first = false;
                }
                Self::Xml(writer) => {
                    writer.write_all(b"  <row>\n").map_err(io_error)?;
                    for (index, name) in columns.iter().enumerate() {
                        writeln!(
                            writer,
                            "    <col name=\"{}\">{}</col>",
                            escape_xml(name),
                            escape_xml(&cell_string(row.get(index)))
                        )
                        .map_err(io_error)?;
                    }
                    writer.write_all(b"  </row>\n").map_err(io_error)?;
                }
            }
        }
        Ok(())
    }

    fn finish(mut self) -> AppResult<()> {
        match &mut self {
            Self::Delimited(writer) => writer.flush().map_err(io_error),
            Self::Json(writer, _) => {
                writer.write_all(b"\n]\n").map_err(io_error)?;
                writer.flush().map_err(io_error)
            }
            Self::Xml(writer) => {
                writer.write_all(b"</rows>\n").map_err(io_error)?;
                writer.flush().map_err(io_error)
            }
        }
    }
}

fn io_error(error: impl std::fmt::Display) -> AppError {
    AppError::msg(error.to_string())
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
