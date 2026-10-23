import { Table } from "antd";
import type { ColumnsType } from "antd/es/table";
import { memo, useMemo } from "react";
import type { JsonValue, StatementResult } from "../types";
import { displayCell, isNumericType } from "../format";

interface ResultRow { key: string; cells: JsonValue[] }

/** Draft edits and operation polling do not rebuild an unchanged result grid. */
export const QueryResultTable = memo(function QueryResultTable({ statement }: { statement: StatementResult }) {
  const columns = useMemo<ColumnsType<ResultRow>>(() => statement.columns.map((column, index) => ({
    title: column.name, key: `${column.name}-${index}`, width: 180,
    align: isNumericType(column.dataType) ? "right" : "left",
    render: (_value, row) => {
      const cell = row.cells[index];
      return <span className={cell === null ? "xb-null" : isNumericType(column.dataType) ? "xb-num" : undefined}>{displayCell(cell, column.dataType)}</span>;
    },
  })), [statement.columns]);
  const rows = useMemo(() => statement.rows.map((cells, index) => ({ key: `${statement.page}-${index}`, cells })), [statement.rows, statement.page]);
  return <Table<ResultRow> size="small" pagination={false} columns={columns} dataSource={rows}
    virtual={rows.length > 200} sticky scroll={{ x: Math.max(720, columns.length * 180), y: 420 }} />;
});
