use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use serde_json::{json, Value as JsonValue};

use crate::error::{AppError, AppResult};

pub fn write_dbf(path: &Path, columns: &[String], rows: &[Vec<JsonValue>]) -> AppResult<()> {
    if columns.len() > 128 {
        return Err(AppError::msg("DBF 最多 128 列"));
    }
    let fields: Vec<(String, u8)> = columns
        .iter()
        .map(|name| {
            let mut bytes = name.as_bytes().to_vec();
            bytes.truncate(11);
            let text = String::from_utf8_lossy(&bytes).to_string();
            (text, 48u8)
        })
        .collect();
    let header_len = 32 + fields.len() * 32 + 1;
    let record_len = 1 + fields.len() * 48;
    let mut file = File::create(path).map_err(|err| AppError::msg(err.to_string()))?;
    let mut header = vec![0u8; header_len];
    header[0] = 0x03;
    let count = rows.len() as u32;
    header[4..8].copy_from_slice(&count.to_le_bytes());
    header[8..10].copy_from_slice(&(header_len as u16).to_le_bytes());
    header[10..12].copy_from_slice(&(record_len as u16).to_le_bytes());
    for (index, (name, width)) in fields.iter().enumerate() {
        let start = 32 + index * 32;
        let bytes = name.as_bytes();
        header[start..start + bytes.len()].copy_from_slice(bytes);
        header[start + 11] = b'C';
        header[start + 16] = *width;
    }
    header[header_len - 1] = 0x0D;
    file.write_all(&header).map_err(|err| AppError::msg(err.to_string()))?;
    for row in rows {
        let mut record = vec![b' '; record_len];
        record[0] = b' ';
        for (index, _field) in fields.iter().enumerate() {
            let text = match row.get(index) {
                Some(JsonValue::Null) | None => String::new(),
                Some(JsonValue::String(text)) => text.clone(),
                Some(other) => other.to_string(),
            };
            let mut bytes = text.into_bytes();
            bytes.truncate(48);
            let at = 1 + index * 48;
            record[at..at + bytes.len()].copy_from_slice(&bytes);
        }
        file.write_all(&record).map_err(|err| AppError::msg(err.to_string()))?;
    }
    file.write_all(&[0x1A]).ok();
    Ok(())
}

pub fn read_dbf(path: &Path) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|err| AppError::msg(err.to_string()))?;
    if bytes.len() < 32 {
        return Err(AppError::msg("DBF 文件太小"));
    }
    let count = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let header_len = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
    let record_len = u16::from_le_bytes(bytes[10..12].try_into().unwrap()) as usize;
    if header_len < 33 || record_len == 0 || bytes.len() < header_len {
        return Err(AppError::msg("DBF 头无效"));
    }
    let mut columns = Vec::new();
    let mut widths = Vec::new();
    let mut offset = 32;
    while offset + 32 <= header_len - 1 {
        let name_bytes = &bytes[offset..offset + 11];
        let name = String::from_utf8_lossy(name_bytes).trim_end_matches('\0').trim().to_string();
        if name.is_empty() {
            break;
        }
        columns.push(name);
        widths.push(bytes[offset + 16] as usize);
        offset += 32;
    }
    let mut rows = Vec::new();
    let mut cursor = header_len;
    for _ in 0..count {
        if cursor + record_len > bytes.len() {
            break;
        }
        let record = &bytes[cursor..cursor + record_len];
        if record[0] == b'*' {
            cursor += record_len;
            continue;
        }
        let mut row = Vec::new();
        let mut at = 1;
        for width in &widths {
            let end = (at + *width).min(record.len());
            let text = String::from_utf8_lossy(&record[at..end]).trim().to_string();
            row.push(if text.is_empty() { JsonValue::Null } else { JsonValue::String(text) });
            at = end;
        }
        rows.push(row);
        cursor += record_len;
    }
    let _ = json!(null);
    Ok((columns, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbf_roundtrip_keeps_text() {
        let path = std::env::temp_dir().join(format!("xb-{}.dbf", uuid::Uuid::new_v4()));
        let columns = vec!["TITLE".into(), "NOTE".into()];
        let rows = vec![vec![JsonValue::String("活着".into()), JsonValue::String("余华".into())]];
        write_dbf(&path, &columns, &rows).unwrap();
        let (read_columns, read_rows) = read_dbf(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(read_columns[0], "TITLE");
        assert_eq!(read_rows[0][0], JsonValue::String("活着".into()));
        assert_eq!(read_rows[0][1], JsonValue::String("余华".into()));
    }
}
