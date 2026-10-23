import type { ColumnMeta, FieldValue, JsonValue, RowChange } from "./types";
import { isBlank, sameValue } from "./format";

export interface GridRow {
  rid: string;
  cells: JsonValue[];
  original: JsonValue[] | null;
  originalFields: FieldValue[] | null;
  status: "clean" | "dirty" | "inserted" | "deleted";
}

export function buildChanges(columns: ColumnMeta[], rows: GridRow[]): RowChange[] {
  const changes: RowChange[] = [];
  for (const row of rows) {
    if (row.status === "inserted") {
      const values: FieldValue[] = [];
      let meaningful = false;
      columns.forEach((column, index) => {
        const value = row.cells[index] ?? null;
        if (column.autoIncrement && isBlank(value)) return;
        if (!isBlank(value)) meaningful = true;
        values.push({ column: column.name, value });
      });
      if (!meaningful) continue;
      changes.push({ kind: "insert", pk: [], values });
    } else if (row.status === "deleted" && row.original) {
      changes.push({ kind: "delete", pk: primaryKey(columns, row.original), values: [], original: row.originalFields ?? originalValues(columns, row.original) });
    } else if (row.status === "dirty" && row.original) {
      const values: FieldValue[] = [];
      columns.forEach((column, index) => {
        const value = row.cells[index] ?? null;
        if (!sameValue(value, row.original?.[index])) values.push({ column: column.name, value });
      });
      if (values.length) changes.push({ kind: "update", pk: primaryKey(columns, row.original), values, original: row.originalFields ?? originalValues(columns, row.original) });
    }
  }
  return changes;
}

function originalValues(columns: ColumnMeta[], original: JsonValue[]): FieldValue[] {
  return columns.map((column, index) => ({ column: column.name, value: original[index] ?? null }));
}

function primaryKey(columns: ColumnMeta[], original: JsonValue[]): FieldValue[] {
  return columns
    .map((column, index) => ({ column, index }))
    .filter((item) => item.column.primaryKey)
    .map((item) => ({ column: item.column.name, value: original[item.index] ?? null }));
}
