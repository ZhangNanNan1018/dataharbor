use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow_array::builder::StringBuilder;
use arrow_array::cast::AsArray;
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde_json::Value as JsonValue;

use crate::error::{AppError, AppResult};

/// Write a table as UTF-8 Parquet. Numbers and booleans are stored as text so a
/// round trip keeps the cell the user saw, including nulls.
pub fn write_table(path: &Path, columns: &[String], rows: &[Vec<JsonValue>]) -> AppResult<()> {
    write_with(path, columns, rows, Compression::SNAPPY)
}

fn write_with(path: &Path, columns: &[String], rows: &[Vec<JsonValue>], compression: Compression) -> AppResult<()> {
    if columns.is_empty() {
        return Err(AppError::msg("没有列，不能写成 Parquet"));
    }
    let fields = columns
        .iter()
        .map(|name| Field::new(name, DataType::Utf8, true))
        .collect::<Vec<_>>();
    let schema = Arc::new(Schema::new(fields));
    let mut arrays = Vec::with_capacity(columns.len());
    for index in 0..columns.len() {
        let mut builder = StringBuilder::new();
        for row in rows {
            match row.get(index) {
                None | Some(JsonValue::Null) => builder.append_null(),
                Some(JsonValue::String(text)) => builder.append_value(text),
                Some(other) => builder.append_value(other.to_string()),
            }
        }
        arrays.push(Arc::new(builder.finish()) as arrow_array::ArrayRef);
    }
    let batch = RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|err| AppError::msg(format!("Parquet 列组装失败: {err}")))?;
    let file = File::create(path).map_err(|err| AppError::msg(err.to_string()))?;
    let props = WriterProperties::builder().set_compression(compression).build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .map_err(|err| AppError::msg(format!("Parquet 写入失败: {err}")))?;
    writer
        .write(&batch)
        .map_err(|err| AppError::msg(format!("Parquet 写入失败: {err}")))?;
    writer
        .close()
        .map_err(|err| AppError::msg(format!("Parquet 关闭失败: {err}")))?;
    Ok(())
}

pub fn read_table(path: &Path) -> AppResult<(Vec<String>, Vec<Vec<JsonValue>>)> {
    let file = File::open(path).map_err(|err| AppError::msg(err.to_string()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|err| AppError::msg(format!("无法读取 Parquet: {err}。本构建解码 Snappy、gzip、zstd 和 lz4。")))?;
    let schema = builder.schema().clone();
    let headers = schema.fields().iter().map(|field| field.name().clone()).collect::<Vec<_>>();
    let reader = builder
        .build()
        .map_err(|err| AppError::msg(format!("无法读取 Parquet: {err}")))?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|err| AppError::msg(format!("Parquet 批次损坏: {err}")))?;
        for row_index in 0..batch.num_rows() {
            let mut cells = Vec::with_capacity(batch.num_columns());
            for column in batch.columns() {
                cells.push(cell_at(column.as_ref(), row_index));
            }
            rows.push(cells);
        }
    }
    Ok((headers, rows))
}

fn cell_at(array: &dyn Array, index: usize) -> JsonValue {
    if array.is_null(index) {
        return JsonValue::Null;
    }
    let text = match array.data_type() {
        DataType::Utf8 => array.as_string::<i32>().value(index).to_string(),
        DataType::LargeUtf8 => array.as_string::<i64>().value(index).to_string(),
        DataType::Boolean => array.as_boolean().value(index).to_string(),
        DataType::Int8 => array.as_primitive::<arrow_array::types::Int8Type>().value(index).to_string(),
        DataType::Int16 => array.as_primitive::<arrow_array::types::Int16Type>().value(index).to_string(),
        DataType::Int32 => array.as_primitive::<arrow_array::types::Int32Type>().value(index).to_string(),
        DataType::Int64 => array.as_primitive::<arrow_array::types::Int64Type>().value(index).to_string(),
        DataType::UInt8 => array.as_primitive::<arrow_array::types::UInt8Type>().value(index).to_string(),
        DataType::UInt16 => array.as_primitive::<arrow_array::types::UInt16Type>().value(index).to_string(),
        DataType::UInt32 => array.as_primitive::<arrow_array::types::UInt32Type>().value(index).to_string(),
        DataType::UInt64 => array.as_primitive::<arrow_array::types::UInt64Type>().value(index).to_string(),
        DataType::Float32 => array.as_primitive::<arrow_array::types::Float32Type>().value(index).to_string(),
        DataType::Float64 => array.as_primitive::<arrow_array::types::Float64Type>().value(index).to_string(),
        _ => format!("{:?}", array.slice(index, 1)),
    };
    JsonValue::String(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parquet_round_trip_keeps_nulls_and_chinese() {
        let dir = std::env::temp_dir().join(format!("xb-parquet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("books.parquet");
        let columns = vec!["title".into(), "author".into(), "price".into()];
        let rows = vec![
            vec![
                JsonValue::String("活着".into()),
                JsonValue::String("余华".into()),
                JsonValue::Null,
            ],
            vec![
                JsonValue::String("骆驼祥子".into()),
                JsonValue::String("老舍".into()),
                JsonValue::Number(32.into()),
            ],
        ];
        write_table(&path, &columns, &rows).unwrap();
        let (headers, loaded) = read_table(&path).unwrap();
        assert_eq!(headers, columns);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0][0], JsonValue::String("活着".into()));
        assert_eq!(loaded[0][1], JsonValue::String("余华".into()));
        assert_eq!(loaded[0][2], JsonValue::Null);
        assert_eq!(loaded[1][2], JsonValue::String("32".into()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parquet_zstd_and_lz4_round_trip() {
        let dir = std::env::temp_dir().join(format!("xb-parquet-codec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let columns = vec!["title".into()];
        let rows = vec![vec![JsonValue::String("活着".into())]];
        for (name, compression) in [
            ("zstd", Compression::ZSTD(parquet::basic::ZstdLevel::default())),
            ("lz4", Compression::LZ4_RAW),
        ] {
            let path = dir.join(format!("{name}.parquet"));
            write_with(&path, &columns, &rows, compression).unwrap();
            let (headers, loaded) = read_table(&path).unwrap();
            assert_eq!(headers, columns);
            assert_eq!(loaded[0][0], JsonValue::String("活着".into()));
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
