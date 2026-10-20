import {
  App,
  Alert,
  Button,
  Checkbox,
  Input,
  Modal,
  Pagination,
  Select,
  Space,
  Table,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import { useEffect, useRef, useState } from "react";
import { api, errorText, pickCsvFile, pickSaveCsv } from "../api";
import type { ForeignKeyRef } from "../api";
import { CellValue } from "./CellEditors";
import { isBlank, isNumericType, sameValue } from "../format";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";
import type { ColumnMeta, DataTab, FieldValue, FilterSpec, JsonValue, RowChange, SortSpec } from "../types";

interface GridRow {
  rid: string;
  cells: JsonValue[];
  original: JsonValue[] | null;
  status: "clean" | "dirty" | "inserted" | "deleted";
}

const FILTER_OPS = [
  { value: "contains", label: "包含" },
  { value: "eq", label: "等于" },
  { value: "ne", label: "不等于" },
  { value: "gt", label: "大于" },
  { value: "lt", label: "小于" },
  { value: "gte", label: "大于等于" },
  { value: "lte", label: "小于等于" },
  { value: "is_null", label: "为空" },
  { value: "is_not_null", label: "非空" },
];

export function DataGridPanel({ tab }: { tab: DataTab }) {
  const { message, modal } = App.useApp();
  const setStatus = useUiStore((state) => state.setStatus);
  const setDirty = useTabStore((state) => state.setDirty);
  const [columns, setColumns] = useState<ColumnMeta[]>([]);
  const [rows, setRows] = useState<GridRow[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(100);
  const [filters, setFilters] = useState<FilterSpec[]>([]);
  const [sorts, setSorts] = useState<SortSpec[]>([]);
  const [draftColumn, setDraftColumn] = useState<string>();
  const [draftOp, setDraftOp] = useState("contains");
  const [draftValue, setDraftValue] = useState("");
  const [readOnly, setReadOnly] = useState(tab.readOnly);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [importOpen, setImportOpen] = useState(false);
  const [formView, setFormView] = useState(false);
  const [formIndex, setFormIndex] = useState(0);
  const [jsonText, setJsonText] = useState("");
  const [truncate, setTruncate] = useState(false);
  const [foreignKeys, setForeignKeys] = useState<ForeignKeyRef[]>([]);
  const [reviewOpen, setReviewOpen] = useState(false);
  const [csvPath, setCsvPath] = useState("");
  const [importing, setImporting] = useState(false);
  const requestId = useRef(0);
  const rowsRef = useRef<GridRow[]>([]);
  const columnsRef = useRef<ColumnMeta[]>([]);

  const hasPk = columns.some((column) => column.primaryKey);
  const dirty = rows.some((row) => row.status !== "clean");
  rowsRef.current = rows;
  columnsRef.current = columns;
  const needsValue = draftOp !== "is_null" && draftOp !== "is_not_null";

  useEffect(() => {
    setDirty(tab.id, dirty);
  }, [dirty, setDirty, tab.id]);

  const commitRef = useRef<() => Promise<void>>(async () => {});
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void commitRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    void load({ page: 1, size: pageSize, filters: [], sorts: [], force: true });
    void api
      .foreignKeys(tab.connectionId, tab.database, tab.schema, tab.table)
      .then(setForeignKeys)
      .catch(() => setForeignKeys([]));
    // The grid reloads when the opened object changes. Filters stay local to that visit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.connectionId, tab.database, tab.schema, tab.table]);

  async function load(input: {
    page: number;
    size: number;
    filters: FilterSpec[];
    sorts: SortSpec[];
    force?: boolean;
  }) {
    if (!input.force && rows.some((row) => row.status !== "clean")) {
      const ok = await confirmDiscard(modal);
      if (!ok) return;
    }
    const token = ++requestId.current;
    setLoading(true);
    setError("");
    try {
      const data = await api.fetchRows({
        connectionId: tab.connectionId,
        database: tab.database,
        schema: tab.schema,
        table: tab.table,
        filters: input.filters,
        sorts: input.sorts,
        page: input.page,
        pageSize: input.size,
      });
      if (token !== requestId.current) return;
      setColumns(data.columns);
      setRows(
        data.rows.map((cells) => ({
          rid: crypto.randomUUID(),
          cells: [...cells],
          original: cells,
          status: "clean",
        })),
      );
      setTotal(data.total);
      setPage(data.page);
      setPageSize(data.pageSize);
      setReadOnly(tab.readOnly || data.readOnly);
      setSelected([]);
      setStatus(`${tab.table} · ${data.total} 行`);
    } catch (err) {
      if (token !== requestId.current) return;
      setError(errorText(err));
    } finally {
      if (token === requestId.current) setLoading(false);
    }
  }

  function updateCell(rid: string, index: number, value: JsonValue) {
    const next = rowsRef.current.map((row) => {
      if (row.rid !== rid || row.status === "deleted") return row;
      const cells = row.cells.map((cell, cellIndex) => (cellIndex === index ? value : cell));
      if (!row.original || row.status === "inserted") return { ...row, cells, status: "inserted" as const };
      const changed = cells.some((cell, cellIndex) => !sameValue(cell, row.original?.[cellIndex]));
      return { ...row, cells, status: changed ? ("dirty" as const) : ("clean" as const) };
    });
    rowsRef.current = next;
    setRows(next);
  }

  function addRow() {
    if (!columns.length) return;
    setRows((prev) => [
      {
        rid: crypto.randomUUID(),
        cells: columns.map(() => null),
        original: null,
        status: "inserted",
      },
      ...prev,
    ]);
  }

  function deleteSelected() {
    if (!selected.length) return;
    if (!hasPk) {
      const onlyNew = rows.filter((row) => selected.includes(row.rid)).every((row) => row.status === "inserted");
      if (!onlyNew) {
        message.warning("没有主键，不能删除已有行");
      }
    }
    setRows((prev) =>
      prev.flatMap((row) => {
        if (!selected.includes(row.rid)) return [row];
        if (row.status === "inserted") return [];
        if (!hasPk) return [row];
        return [{ ...row, status: "deleted" as const }];
      }),
    );
    setSelected([]);
  }

  function cycleSort(column: string) {
    const current = sorts[0];
    let next: SortSpec[] = [];
    if (!current || current.column !== column) next = [{ column, desc: false }];
    else if (!current.desc) next = [{ column, desc: true }];
    setSorts(next);
    void load({ page: 1, size: pageSize, filters, sorts: next });
  }

  function addFilter() {
    if (!draftColumn) {
      message.warning("先选择筛选列");
      return;
    }
    if (needsValue && !draftValue.trim()) {
      message.warning("填写筛选值");
      return;
    }
    if (filters.length >= 12) {
      message.warning("筛选条件最多 12 条");
      return;
    }
    const next = [
      ...filters,
      { column: draftColumn, op: draftOp, value: needsValue ? draftValue : null },
    ];
    setFilters(next);
    setDraftValue("");
    void load({ page: 1, size: pageSize, filters: next, sorts });
  }

  function removeFilter(index: number) {
    const next = filters.filter((_, item) => item !== index);
    setFilters(next);
    void load({ page: 1, size: pageSize, filters: next, sorts });
  }

  async function commit() {
    const active = document.activeElement;
    if (active instanceof HTMLElement && active !== document.body) active.blur();
    const changes = buildChanges(columnsRef.current, rowsRef.current);
    if (!changes.length) {
      setStatus("没有需要提交的更改");
      message.info("没有需要提交的更改");
      return;
    }
    setLoading(true);
    setError("");
    try {
      const result = await api.commitChanges({
        connectionId: tab.connectionId,
        database: tab.database,
        schema: tab.schema,
        table: tab.table,
        changes,
      });
      const note = `已提交 ${result.applied} 条更改`;
      setStatus(note);
      message.success(note);
      await load({ page, size: pageSize, filters, sorts, force: true });
    } catch (err) {
      const text = errorText(err);
      setError(text);
      setStatus(text);
      message.error(text);
      setLoading(false);
    }
  }
  commitRef.current = commit;

  async function exportCsv() {
    const path = await pickSaveCsv(`${tab.table}.csv`);
    if (!path) return;
    try {
      const exported = await api.exportTableCsv({
        connectionId: tab.connectionId,
        database: tab.database,
        schema: tab.schema,
        table: tab.table,
        filters,
        sorts,
        path,
      });
      const text = `已导出 ${exported.rowCount} 行${exported.truncated ? "，结果已截断，仅包含部分数据" : ""}`;
      if (exported.truncated) message.warning(text);
      else message.success(text);
      setStatus(`CSV 已写入 ${exported.path}`);
    } catch (err) {
      message.error(errorText(err));
    }
  }

  async function importCsv() {
    if (!csvPath) {
      message.warning("先选择 CSV 文件");
      return;
    }
    if (truncate) {
      const ok = await new Promise<boolean>((resolve) => {
        modal.confirm({
          title: "导入前清空这张表？",
          content: "会删除表中现有行，再写入 CSV。",
          okText: "清空并导入",
          okButtonProps: { danger: true },
          cancelText: "取消",
          onOk: () => resolve(true),
          onCancel: () => resolve(false),
        });
      });
      if (!ok) return;
    }
    setImporting(true);
    try {
      const result = await api.importTableCsv({
        connectionId: tab.connectionId,
        database: tab.database,
        schema: tab.schema,
        table: tab.table,
        path: csvPath,
        truncate,
      });
      message.success(`已导入 ${result.inserted} 行`);
      setImportOpen(false);
      setCsvPath("");
      setTruncate(false);
      await load({ page: 1, size: pageSize, filters, sorts, force: true });
    } catch (err) {
      message.error(errorText(err));
    } finally {
      setImporting(false);
    }
  }

  const tableColumns: ColumnsType<GridRow> = columns.map((column, index) => ({
    title: (
      <button type="button" className="xb-sort" onClick={() => cycleSort(column.name)}>
        {column.name}
        {column.primaryKey ? " · PK" : ""}
        {sorts[0]?.column === column.name ? (sorts[0].desc ? " ↓" : " ↑") : ""}
      </button>
    ),
    key: `${column.name}-${index}`,
    align: isNumericType(column.dataType) ? "right" : "left",
    render: (_value, row) => (
      <CellValue
        column={column}
        value={row.cells[index] ?? null}
        dirty={Boolean(row.original) && !sameValue(row.cells[index], row.original?.[index])}
        disabled={readOnly || row.status === "deleted" || (row.status !== "inserted" && !hasPk)}
        fk={foreignKeys.find((item) => item.column.toLowerCase() === column.name.toLowerCase())}
        connectionId={tab.connectionId}
        database={tab.database}
        onCommit={(value) => updateCell(row.rid, index, value)}
      />
    ),
  }));

  return (
    <div className="xb-panel">
      <div className="flex flex-wrap items-center gap-2">
        <strong className="text-sm">{tab.table}</strong>
        <span className="text-xs text-[var(--xb-muted)]">
          {[tab.database, tab.schema].filter(Boolean).join(" / ")}
        </span>
        <div className="flex-1" />
        <Button size="small" onClick={() => void load({ page, size: pageSize, filters, sorts })} loading={loading}>
          刷新
        </Button>
        <Button size="small" disabled={readOnly || !columns.length} onClick={addRow}>
          新增行
        </Button>
        <Button size="small" danger disabled={readOnly || !selected.length} onClick={deleteSelected}>
          删除
        </Button>
        <Button size="small" disabled={!dirty} onClick={() => setReviewOpen((value) => !value)}>
          查看更改
        </Button>
        <Button size="small" type="primary" disabled={readOnly} onClick={() => void commit()}>
          提交
        </Button>
        <Button size="small" onClick={() => void exportCsv()}>
          导出 CSV
        </Button>
        <Button size="small" disabled={readOnly} onClick={() => setImportOpen(true)}>
          导入 CSV
        </Button>
        <Button size="small" onClick={() => setFormView((value) => !value)}>
          {formView ? "网格" : "表单"}
        </Button>
        <Button
          size="small"
          onClick={() => {
            void api
              .profileTable({ connectionId: tab.connectionId, database: tab.database, schema: tab.schema, table: tab.table })
              .then((grid) => setJsonText([grid.note, grid.columns.join(" | "), ...grid.rows.map((row) => row.map((cell) => String(cell ?? "")).join(" | "))].join("\n")))
              .catch((err) => message.error(errorText(err)));
          }}
        >
          画像
        </Button>
        <Button
          size="small"
          disabled={readOnly}
          onClick={() => {
            const row = rows[formIndex] ?? rows[0];
            if (!row) return;
            const schemaText = columns.map((column) => `${column.name} ${column.dataType}`).join(", ");
            const current = Object.fromEntries(columns.map((column, index) => [column.name, row.cells[index]]));
            void api
              .aiAsk({ task: "fill", sql: JSON.stringify(current), schemaText, prompt: "只补空值", engine: "sqlite" })
              .then((text) => {
                const match = text.match(/\{[\s\S]*\}/);
                if (!match) throw new Error("模型没有返回 JSON 对象");
                const object = JSON.parse(match[0]) as Record<string, JsonValue>;
                columns.forEach((column, index) => {
                  if (row.cells[index] == null && object[column.name] !== undefined) updateCell(row.rid, index, object[column.name]);
                });
                message.success("已写入未保存的补全，提交后才会落库");
              })
              .catch((err) => message.error(errorText(err)));
          }}
        >
          AI 填充
        </Button>
        <span className="text-xs text-[var(--xb-muted)]">AI 填充会将当前行和表结构发送至你配置的 AI 接口</span>
        <Button
          size="small"
          onClick={() => {
            const row = rows.find((item) => selected.includes(item.rid)) ?? rows[0];
            if (!row) return;
            const object: Record<string, JsonValue> = {};
            columns.forEach((column, index) => {
              object[column.name] = row.cells[index] ?? null;
            });
            setJsonText(JSON.stringify(object, null, 2));
          }}
        >
          JSON
        </Button>
      </div>
      {dirty && (
        <div className="xb-review-bar">
          <strong>{buildChanges(columns, rows).length} 处待提交</strong>
          <span className="text-xs text-[var(--xb-muted)]">改动先停在这里，提交后才写入数据库。</span>
          <div className="flex-1" />
          <Button size="small" onClick={() => setReviewOpen(true)}>查看</Button>
          <Button size="small" onClick={() => void load({ page, size: pageSize, filters, sorts, force: true })}>放弃</Button>
          <Button size="small" type="primary" onClick={() => void commit()}>提交</Button>
        </div>
      )}
      {reviewOpen && dirty && (
        <div className="xb-review">
          {buildChanges(columns, rows).map((change, index) => (
            <div key={`${change.kind}-${index}`} className="xb-review-row">
              <span className="xb-review-kind">{change.kind === "insert" ? "新增" : change.kind === "delete" ? "删除" : "更新"}</span>
              <span>
                {change.values.map((field) => `${field.column}=${field.value === null ? "NULL" : String(field.value)}`).join(", ") || change.pk.map((field) => field.column).join(", ")}
              </span>
            </div>
          ))}
          {buildChanges(columns, rows).length === 0 && <div>有未完成的空行，还没有可提交的值。</div>}
        </div>
      )}
      {formView && rows.length > 0 && (
        <div className="space-y-2">
          <Space>
            <Button size="small" disabled={formIndex <= 0} onClick={() => setFormIndex((value) => Math.max(0, value - 1))}>上一条</Button>
            <span className="text-xs text-[var(--xb-muted)]">{formIndex + 1} / {rows.length}</span>
            <Button size="small" disabled={formIndex >= rows.length - 1} onClick={() => setFormIndex((value) => Math.min(rows.length - 1, value + 1))}>下一条</Button>
          </Space>
          <div className="grid gap-2 md:grid-cols-2">
            {columns.map((column, index) => {
              const row = rows[Math.min(formIndex, rows.length - 1)];
              return (
                <label key={column.name} className="text-xs">
                  <span className="text-[var(--xb-muted)]">{column.name}</span>
                  <CellValue
                    alwaysEdit
                    column={column}
                    value={row.cells[index] ?? null}
                    dirty={Boolean(row.original) && !sameValue(row.cells[index], row.original?.[index])}
                    disabled={readOnly || row.status === "deleted"}
                    fk={foreignKeys.find((item) => item.column.toLowerCase() === column.name.toLowerCase())}
                    connectionId={tab.connectionId}
                    database={tab.database}
                    onCommit={(value) => updateCell(row.rid, index, value)}
                  />
                </label>
              );
            })}
          </div>
        </div>
      )}
      {jsonText && <Input.TextArea rows={8} value={jsonText} readOnly />}
      <div className="flex flex-wrap items-center gap-2">
        <Select
          size="small"
          placeholder="列"
          style={{ width: 140 }}
          value={draftColumn}
          options={columns.map((column) => ({ value: column.name, label: column.name }))}
          onChange={setDraftColumn}
        />
        <Select size="small" style={{ width: 120 }} value={draftOp} options={FILTER_OPS} onChange={setDraftOp} />
        {needsValue && (
          <Input
            size="small"
            style={{ width: 180 }}
            placeholder="筛选值"
            value={draftValue}
            onChange={(event) => setDraftValue(event.target.value)}
            onPressEnter={addFilter}
          />
        )}
        <Button size="small" onClick={addFilter}>
          添加筛选
        </Button>
        {filters.map((filter, index) => (
          <Button key={`${filter.column}-${filter.op}-${index}`} size="small" onClick={() => removeFilter(index)}>
            {filter.column} {opLabel(filter.op)} {filter.value ?? ""} ×
          </Button>
        ))}
      </div>
      {readOnly && <Alert type="info" showIcon message="这个对象是只读的，不能改数据。" />}
      {!readOnly && !hasPk && columns.length > 0 && (
        <Alert type="warning" showIcon message="没有主键，不能更新或删除已有行。仍可以新增行。" />
      )}
      {error && <Alert type="error" showIcon message={error} />}
      <div className="xb-grid-wrap">
        <Table<GridRow>
          size="small"
          rowKey="rid"
          loading={loading}
          pagination={false}
          columns={tableColumns}
          dataSource={rows}
          sticky
          scroll={{ x: "max-content" }}
          rowClassName={(row) =>
            row.status === "deleted" ? "xb-row-deleted" : row.status === "inserted" ? "xb-row-inserted" : ""
          }
          rowSelection={{
            selectedRowKeys: selected,
            onChange: (keys) => setSelected(keys.map(String)),
          }}
        />
      </div>
      <div className="xb-result-meta">
        <span className="text-xs text-[var(--xb-muted)]">
          {dirty ? "有未提交的更改" : `${total} 行`}
        </span>
        <Pagination
          size="small"
          current={page}
          pageSize={pageSize}
          total={total}
          showSizeChanger
          pageSizeOptions={[50, 100, 200, 500]}
          disabled={loading}
          onChange={(next, size) => void load({ page: next, size, filters, sorts })}
        />
      </div>
      <Modal
        title={`导入 CSV 到 ${tab.table}`}
        open={importOpen}
        okText="导入"
        cancelText="取消"
        confirmLoading={importing}
        onOk={() => void importCsv()}
        onCancel={() => setImportOpen(false)}
      >
        <Space direction="vertical" className="w-full">
          <div className="text-sm text-[var(--xb-muted)]">第一行是表头，空单元格写入 NULL。单次最多 10 万行、50MB。</div>
          <Space.Compact className="w-full">
            <Input value={csvPath} placeholder="选择 CSV 文件" readOnly />
            <Button
              onClick={() => {
                void pickCsvFile().then((path) => {
                  if (path) setCsvPath(path);
                });
              }}
            >
              浏览
            </Button>
          </Space.Compact>
          <Checkbox checked={truncate} onChange={(event) => setTruncate(event.target.checked)}>
            导入前删除表中现有行
          </Checkbox>
        </Space>
      </Modal>
    </div>
  );
}

function buildChanges(columns: ColumnMeta[], rows: GridRow[]): RowChange[] {
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
      changes.push({ kind: "delete", pk: primaryKey(columns, row.original), values: [] });
    } else if (row.status === "dirty" && row.original) {
      const values: FieldValue[] = [];
      columns.forEach((column, index) => {
        const value = row.cells[index] ?? null;
        if (!sameValue(value, row.original?.[index])) values.push({ column: column.name, value });
      });
      if (values.length) changes.push({ kind: "update", pk: primaryKey(columns, row.original), values });
    }
  }
  return changes;
}

function primaryKey(columns: ColumnMeta[], original: JsonValue[]): FieldValue[] {
  return columns
    .map((column, index) => ({ column, index }))
    .filter((item) => item.column.primaryKey)
    .map((item) => ({ column: item.column.name, value: original[item.index] ?? null }));
}

function opLabel(op: string): string {
  return FILTER_OPS.find((item) => item.value === op)?.label ?? op;
}

function confirmDiscard(modal: ReturnType<typeof App.useApp>["modal"]): Promise<boolean> {
  return new Promise((resolve) => {
    modal.confirm({
      title: "放弃未提交的修改？",
      content: "刷新或翻页会丢掉当前页还没提交的增删改。",
      okText: "放弃并继续",
      cancelText: "留下",
      onOk: () => resolve(true),
      onCancel: () => resolve(false),
    });
  });
}
