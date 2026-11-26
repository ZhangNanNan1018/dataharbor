//! MySQL and MariaDB wire values which need column-aware decoding.
use geozero::{wkb::Wkb, ToWkt};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::models::Engine;
use crate::sqlutil::{binary_cell_bytes, blob_literal, json_blob};

pub fn bit_width(kind: &str) -> Option<u32> {
    let kind = kind.trim().to_ascii_lowercase();
    if kind == "bit" {
        return Some(1);
    }
    kind.strip_prefix("bit(")?
        .strip_suffix(')')?
        .parse()
        .ok()
        .filter(|n| (1..=64).contains(n))
}

pub fn bit_literal(width: u32, value: &Value) -> AppResult<String> {
    let number = value
        .as_u64()
        .or_else(|| {
            let text = value.as_str()?.trim();
            if text.bytes().all(|c| c.is_ascii_digit()) {
                text.parse().ok()
            } else {
                None
            }
        })
        .filter(|n| width == 64 || *n < (1_u64 << width))
        .ok_or_else(|| AppError::msg(format!("BIT({width}) 需要位宽范围内的无符号十进制整数")))?;
    Ok(number.to_string())
}

// Previously these snapshots were the UTF-8 interpretation of the wire bytes
// (or a base64 blob for invalid UTF-8). New small BIT values are JSON numbers.
pub fn original_literal(engine: Engine, kind: &str, value: &Value) -> AppResult<String> {
    if matches!(engine, Engine::Mysql | Engine::Mariadb) {
        if let Some(text) = value.as_str() {
            if let Some(width) = bit_width(kind) {
                let length = width.div_ceil(8) as usize;
                let bytes = if text.len() == length {
                    Some(text.as_bytes().to_vec())
                } else {
                    binary_cell_bytes(value).filter(|b| b.len() == length)
                };
                if let Some(bytes) = bytes {
                    let mut word = [0; 8];
                    word[8 - length..].copy_from_slice(&bytes);
                    return bit_literal(width, &json!(u64::from_be_bytes(word)));
                }
            } else if is_geometry(kind) {
                let bytes = binary_cell_bytes(value).unwrap_or_else(|| text.as_bytes().to_vec());
                let cell = geometry_cell(&bytes)
                    .map_err(|_| AppError::msg("旧空间字段快照无法还原，请重新加载"))?;
                return geometry_literal(engine, &cell);
            }
        }
    }
    crate::sqlutil::value_sql(engine, kind, value)
}

pub fn is_geometry(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "geometry"
            | "point"
            | "linestring"
            | "polygon"
            | "multipoint"
            | "multilinestring"
            | "multipolygon"
            | "geometrycollection"
            | "geomcollection"
    )
}

pub fn geometry_cell(bytes: &[u8]) -> AppResult<Value> {
    let (prefix, wkb) = bytes
        .split_at_checked(4)
        .ok_or_else(|| AppError::msg("空间值缺少 SRID"))?;
    let srid = u32::from_le_bytes(prefix.try_into().unwrap());
    let wkt = Wkb(wkb)
        .to_wkt()
        .map_err(|e| AppError::msg(format!("空间字段 WKB 无效：{e}")))?;
    Ok(json!({"srid": srid, "wkt": wkt, "wkb": json_blob(wkb)}))
}

pub fn geometry_literal(engine: Engine, value: &Value) -> AppResult<String> {
    let (srid, wkt) = if let Some(text) = value.as_str() {
        match text.trim().strip_prefix("SRID=") {
            Some(ewkt) => {
                let (id, wkt) = ewkt
                    .split_once(';')
                    .ok_or_else(|| AppError::msg("空间字段需要 SRID=编号;WKT"))?;
                (
                    id.parse::<u32>().map_err(|_| AppError::msg("SRID 无效"))?,
                    wkt,
                )
            }
            None => (0, text),
        }
    } else {
        let srid = value["srid"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| AppError::msg("空间字段缺少有效 SRID"))?;
        (
            srid,
            value["wkt"]
                .as_str()
                .ok_or_else(|| AppError::msg("空间字段缺少 WKT"))?,
        )
    };
    // MySQL stores geographic X/Y as longitude/latitude. MariaDB uses X/Y directly.
    let axis = if engine == Engine::Mysql && srid != 0 {
        ", 'axis-order=long-lat'"
    } else {
        ""
    };
    if let Some(raw) = value.get("wkb") {
        let bytes = binary_cell_bytes(raw).ok_or_else(|| AppError::msg("空间字段原始 WKB 无效"))?;
        let literal = blob_literal(engine, &bytes)?;
        return Ok(format!("ST_GeomFromWKB({literal}, {srid}{axis})"));
    }
    let text = blob_literal(engine, wkt.as_bytes())?;
    Ok(format!(
        "ST_GeomFromText(CONVERT({text} USING utf8mb4), {srid}{axis})"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geozero::{wkt::Wkt, CoordDimensions, ToWkb};

    #[test]
    fn preserves_geometry_families_and_xy_order() {
        for wkt in [
            "POINT(121 31)",
            "LINESTRING(0 1,2 3)",
            "POLYGON((0 0,2 1,1 3,0 0))",
            "MULTIPOINT(0 1,2 3)",
            "MULTILINESTRING((0 1,2 3))",
            "MULTIPOLYGON(((0 0,2 1,1 3,0 0)))",
            "GEOMETRYCOLLECTION(POINT(2 3),LINESTRING(0 1,2 3))",
            "GEOMETRYCOLLECTION EMPTY",
        ] {
            let bytes = Wkt(wkt).to_wkb(CoordDimensions::xy()).unwrap();
            let cell = geometry_cell(&[4326u32.to_le_bytes().as_slice(), &bytes].concat()).unwrap();
            assert_eq!(cell["srid"], 4326);
            let display = cell["wkt"].as_str().unwrap();
            assert_eq!(Wkt(display).to_wkb(CoordDimensions::xy()).unwrap(), bytes);
            assert_eq!(binary_cell_bytes(&cell["wkb"]).unwrap(), bytes);
        }
    }

    #[test]
    fn old_bit_snapshots_recover_wire_bytes_and_keep_new_large_decimal_values() {
        for (width, value, decimal) in [
            (1, json!("\u{1}"), "1"),
            (8, json!("A"), "65"),
            (8, json!("1"), "49"),
            (8, json!("base64:/w=="), "255"),
            (16, json!(256), "256"),
            (64, json!(u64::MAX.to_string()), "18446744073709551615"),
        ] {
            assert_eq!(
                original_literal(Engine::Mysql, &format!("bit({width})"), &value).unwrap(),
                decimal
            );
        }
    }

    #[test]
    fn bit_literals_reject_overflow_and_fractional_values() {
        for bad in [json!("256"), json!("-1"), json!(1.5), json!("1e2")] {
            assert!(bit_literal(8, &bad).is_err());
        }
        assert_eq!(
            bit_literal(64, &json!(u64::MAX.to_string())).unwrap(),
            u64::MAX.to_string()
        );
    }
}
