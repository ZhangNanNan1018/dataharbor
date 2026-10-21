//! Shared CSV/TSV encoding. Empty text stays empty; \N denotes NULL. Only
//! strings made of backslashes followed by N are escaped; UNC paths stay intact.
use crate::error::{AppError, AppResult};
use crate::operations::OperationContext;
use serde_json::Value;
use std::io::{Read, Write};

pub(crate) fn cell(value: &Value) -> String {
    match value {
        Value::Null => "\\N".into(),
        Value::String(text) if null_marker_family(text) => format!("\\{text}"),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn parse_cell(text: &str) -> Value {
    if text == "\\N" {
        Value::Null
    } else if null_marker_family(text) {
        Value::String(text[1..].to_string())
    } else {
        Value::String(text.to_string())
    }
}

fn null_marker_family(text: &str) -> bool {
    text.strip_suffix('N')
        .is_some_and(|prefix| !prefix.is_empty() && prefix.bytes().all(|byte| byte == b'\\'))
}

pub(crate) fn read(
    reader: impl Read,
    delimiter: u8,
    max_rows: usize,
) -> AppResult<(Vec<String>, Vec<Vec<Value>>)> {
    read_with_operation(reader, delimiter, max_rows, &OperationContext::detached())
}

pub(crate) fn read_with_operation(
    reader: impl Read,
    delimiter: u8,
    max_rows: usize,
    operation: &OperationContext,
) -> AppResult<(Vec<String>, Vec<Vec<Value>>)> {
    operation.checkpoint()?;
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .from_reader(reader);
    let headers = reader
        .headers()
        .map_err(|err| AppError::msg(format!("无法解析表头：{err}")))?
        .iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut seen = std::collections::BTreeSet::new();
    if headers.is_empty()
        || headers
            .iter()
            .any(|name| name.trim().is_empty() || !seen.insert(name.to_lowercase()))
    {
        return Err(AppError::msg("表头不能为空或重复"));
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        operation.checkpoint()?;
        let record = record.map_err(|err| AppError::msg(format!("无法解析记录：{err}")))?;
        if rows.len() == max_rows {
            return Err(AppError::msg(format!("一次最多导入 {max_rows} 行")));
        }
        rows.push(record.iter().map(parse_cell).collect());
        if rows.len() % 500 == 0 {
            operation.report("解析文件", rows.len() as u64, None, "尚未写入数据库");
        }
    }
    Ok((headers, rows))
}

#[cfg(test)]
pub(crate) fn write(
    writer: impl Write,
    columns: &[String],
    rows: &[Vec<Value>],
    delimiter: u8,
) -> AppResult<()> {
    write_with_operation(
        writer,
        columns,
        rows,
        delimiter,
        &OperationContext::detached(),
    )
}

pub(crate) fn write_with_operation(
    writer: impl Write,
    columns: &[String],
    rows: &[Vec<Value>],
    delimiter: u8,
    operation: &OperationContext,
) -> AppResult<()> {
    operation.checkpoint()?;
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(writer);
    writer
        .write_record(columns)
        .map_err(|err| AppError::msg(err.to_string()))?;
    for (index, row) in rows.iter().enumerate() {
        operation.checkpoint()?;
        writer
            .write_record(row.iter().map(cell))
            .map_err(|err| AppError::msg(err.to_string()))?;
        if index % 500 == 0 {
            operation.report(
                "写出文件",
                index as u64 + 1,
                Some(rows.len() as u64),
                "正在写入临时文件，完成前不替换目标文件",
            );
        }
    }
    writer.flush().map_err(|err| AppError::msg(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn csv_and_tsv_preserve_multiline_quotes_null_empty_and_precise_text() {
        let columns = vec!["内容,\"\n标题".into(), "金额".into()];
        let rows = vec![
            vec![
                json!("a\"b\r\n第二行"),
                json!("12345678901234567890.123456789012345678"),
            ],
            vec![Value::Null, json!("")],
            vec![json!("\\N"), json!("00123")],
            vec![json!("\\\\path"), json!("18446744073709551615")],
        ];
        for sep in [b',', b'\t'] {
            let mut output = Vec::new();
            write(&mut output, &columns, &rows, sep).unwrap();
            assert_eq!(
                read(output.as_slice(), sep, 10).unwrap(),
                (columns.clone(), rows.clone())
            );
        }
    }

    #[test]
    fn rejects_misaligned_duplicate_and_excess_rows() {
        for text in ["id,id\n1,2\n", "id,name\n1\n", "\n"] {
            assert!(read(text.as_bytes(), b',', 10).is_err());
        }
        assert!(read("id\n1\n2".as_bytes(), b',', 1).is_err());
    }

    #[test]
    fn external_unc_paths_are_not_treated_as_escapes() {
        let (_, rows) = read(
            "path\n\\\\server\\share\n\\\\\\folder\n".as_bytes(),
            b',',
            10,
        )
        .unwrap();
        assert_eq!(
            rows,
            vec![
                vec![json!("\\\\server\\share")],
                vec![json!("\\\\\\folder")]
            ]
        );
    }
}
