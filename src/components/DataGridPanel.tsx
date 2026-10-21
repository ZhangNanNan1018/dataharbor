import {
  App,
  Alert,
  Button,
  Checkbox,
  Dropdown,
  Input,
  Modal,
  Pagination,
  Select,
  Space,
  Table,
} from "antd";
import type { ColumnsType } from "antd/es/table";
import { useEffect, useRef, useState } from "react";
import { api, errorText, isOperationCancelled, pickCsvFile, pickSaveCsv } from "../api";
import { useLongOperation } from "../operations";
import { TaskProgress } from "./TaskProgress";
import type { ForeignKeyRef } from "../api";
import { CellValue } from "./CellEditors";
import { isBlank, isNumericType, sameValue } from "../format";
import { useTabStore } from "../stores/useTabStore";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useUiStore } from "../stores/useUiStore";
import type { ColumnMeta, DataTab, FieldValue, FilterSpec, JsonValue, RowChange, SortSpec } from "../types";

interface GridRow {
  rid: string;
  cells: JsonValue[];
  original: JsonValue[] | null;
  originalFields: FieldValue[] | null;
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
  const task = useLongOperation(tab.id);
  const exportInFlight = useRef(false);
  const setStatus = useUiStore((state) => state.setStatus);
  const setDirty = useTabStore((state) => state.setDirty);
  const supportsFilters = useConnectionStore((state) => state.profiles.find((profile) => profile.id === tab.connectionId)?.engine !== "redis");
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
  const [submitting, setSubmitting] = useState(false);
  const [refreshRequired, setRefreshRequired] = useState(false);
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
  const writeInFlight = useRef(false);
  const editsLocked = useRef(false);

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
      if (useTabStore.getState().activeId === tab.id && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        void commitRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [tab.id]);

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
    afterWrite?: boolean;
  }) {
    if (writeInFlight.current && !input.afterWrite) return false;
    if (!input.force && rowsRef.current.some((row) => row.status !== "clean")) {
      const ok = await confirmDiscard(modal);
      if (!ok || writeInFlight.current) return false;
    }
    const token = ++requestId.current;
    const appliedFilters = supportsFilters ? input.filters : [];
    const appliedSorts = supportsFilters ? input.sorts : [];
    setLoading(true);
    setError("");
    try {
      const data = await api.fetchRows({
        connectionId: tab.connectionId,
        database: tab.database,
        schema: tab.schema,
        table: tab.table,
        filters: appliedFilters,
        sorts: appliedSorts,
        page: input.page,
        pageSize: input.size,
      });
      if (token !== requestId.current) return false;
      setColumns(data.columns);
      setRows(
        data.rows.map((cells, index) => ({
          rid: crypto.randomUUID(),
          cells: [...cells],
          original: cells,
          originalFields: data.originalRows?.[index] ?? null,
          status: "clean",
        })),
      );
      setTotal(data.total);
      setPage(data.page);
      setPageSize(data.pageSize);
      setFilters(appliedFilters);
      setSorts(appliedSorts);
      setRefreshRequired(false);
      setReadOnly(tab.readOnly || data.readOnly);
      setSelected([]);
      setStatus(`${tab.table} · ${data.total} 行`);
      return true;
    } catch (err) {
      if (token !== requestId.current) return false;
      setError(errorText(err));
      return false;
    } finally {
      if (token === requestId.current) setLoading(false);
    }
  }

  function updateCell(rid: string, index: number, value: JsonValue) {
    if (editsLocked.current || refreshRequired) return;
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
    if (!columns.length || writeInFlight.current || refreshRequired) return;
    setRows((prev) => [
      {
        rid: crypto.randomUUID(),
        cells: columns.map(() => null),
        original: null,
        originalFields: null,
        status: "inserted",
      },
      ...prev,
    ]);
  }

  function deleteSelected() {
    if (!selected.length || writeInFlight.current || refreshRequired) return;
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
    if (!supportsFilters) return;
    const current = sorts[0];
    let next: SortSpec[] = [];
    if (!current || current.column !== column) next = [{ column, desc: false }];
    else if (!current.desc) next = [{ column, desc: true }];
    void load({ page: 1, size: pageSize, filters, sorts: next });
  }

  function addFilter() {
    if (!supportsFilters) return;
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
    void load({ page: 1, size: pageSize, filters: next, sorts }).then((loaded) => {
      if (loaded) setDraftValue("");
    });
  }

  function removeFilter(index: number) {
    if (!supportsFilters) return;
    const next = filters.filter((_, item) => item !== index);
    void load({ page: 1, size: pageSize, filters: next, sorts });
  }

  async function commit() {
    if (writeInFlight.current || exportInFlight.current || task.active || loading || readOnly || refreshRequired) return;
    writeInFlight.current = true;
    const active = document.activeElement;
    if (active instanceof HTMLElement && active !== document.body) active.blur();
    const changes = buildChanges(columnsRef.current, rowsRef.current);
    if (!changes.length) {
      writeInFlight.current = false;
      setStatus("没有需要提交的更改");
      message.info("没有需要提交的更改");
      return;
    }
    editsLocked.current = true;
    setSubmitting(true);
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
      // A successful write must never be offered for submission again, even if reloading fails.
      const clean = rowsRef.current.filter((row) => row.status !== "deleted").map((row) => ({
        ...row, original: [...row.cells], status: "clean" as const,
      }));
      rowsRef.current = clean;
      setRows(clean);
      setRefreshRequired(true);
      const loaded = await load({ page, size: pageSize, filters, sorts, force: true, afterWrite: true });
      if (!loaded) {
        setStatus(`${note}，重新读取失败，请刷新后继续编辑`);
        message.warning("提交已成功，重新读取失败。请刷新后继续编辑，不要重复提交。");
      }
    } catch (err) {
      if (err && typeof err === "object" && "kind" in err && err.kind === "partialWrite") {
        setRefreshRequired(true);
      }
      const text = errorText(err);
      setError(text);
      setStatus(text);
      message.error(text);
    } finally {
      writeInFlight.current = false;
      editsLocked.current = false;
      setSubmitting(false);
      setLoading(false);
    }
  }
  commitRef.current = commit;

  async function exportCsv() {
    if (exportInFlight.current || writeInFlight.current || task.active) return;
    exportInFlight.current = true;
    try {
      const path = await pickSaveCsv(`${tab.table}.csv`);
      if (!path) return;
      const exported = await task.execute("导出表数据 CSV", (operationId) => api.exportTableCsv({
        connectionId: tab.connectionId,
        database: tab.database,
        schema: tab.schema,
        table: tab.table,
        filters: supportsFilters ? filters : [],
        sorts: supportsFilters ? sorts : [],
        path,
      }, operationId));
      const text = `已导出 ${exported.rowCount} 行${exported.truncated ? "，结果已截断，仅包含部分数据" : ""}`;
      if (exported.truncated) message.warning(text);
      else message.success(text);
      setStatus(`CSV 已写入 ${exported.path}`);
    } catch (err) {
      setStatus(errorText(err));
      if (!isOperationCancelled(err)) message.error(errorText(err));
    } finally {
      exportInFlight.current = false;
    }
  }

  async function importCsv() {
    if (writeInFlight.current || exportInFlight.current || task.active || loading || refreshRequired) return;
    if (!csvPath) {
      message.warning("先选择 CSV 文件");
      return;
    }
    writeInFlight.current = true;
    try {
      if (rowsRef.current.some((row) => row.status !== "clean") && !(await confirmDiscard(modal))) return;
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
      editsLocked.current = true;
      setImporting(true);
      const result = await task.execute("导入 CSV", (operationId) => api.importTableCsv({
        connectionId: tab.connectionId,
        database: tab.database,
        schema: tab.schema,
        table: tab.table,
        path: csvPath,
        truncate,
      }, operationId));
      message.success(`已导入 ${result.inserted} 行`);
      setImportOpen(false);
      setCsvPath("");
      setTruncate(false);
      rowsRef.current = [];
      setRows([]);
      setRefreshRequired(true);
      await load({ page: 1, size: pageSize, filters, sorts, force: true, afterWrite: true });
    } catch (err) {
      setStatus(errorText(err));
      if (isOperationCancelled(err)) {
        const detail = err as { committed?: number };
        if ((detail.committed ?? 0) > 0) setRefreshRequired(true);
      } else message.error(errorText(err));
    } finally {
      writeInFlight.current = false;
      editsLocked.current = false;
      setImporting(false);
    }
  }

  const tableColumns: ColumnsType<GridRow> = columns.map((column, index) => ({
    title: (
      <button type="button" disabled={!supportsFilters || loading || submitting || importing} className="xb-sort" onClick={() => cycleSort(column.name)}>
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
        disabled={readOnly || loading || submitting || importing || refreshRequired || row.status === "deleted" || (row.status !== "inserted" && !hasPk)}
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
        <Button size="small" disabled={readOnly || loading || submitting || importing || refreshRequired || !columns.length} onClick={addRow}>
          新增行
        </Button>
        <Button size="small" danger disabled={readOnly || loading || submitting || importing || refreshRequired || !selected.length} onClick={deleteSelected}>
          删除
        </Button>
        <Button size="small" disabled={!dirty} onClick={() => setReviewOpen((value) => !value)}>
          查看更改
        </Button>
        <Button size="small" type="primary" loading={submitting} disabled={readOnly || loading || importing || task.active || refreshRequired} onClick={() => void commit()}>
          提交
        </Button>
        <Button size="small" disabled={task.active || loading || submitting || importing} onClick={() => void exportCsv()}>
          导出 CSV
        </Button>
        <Button size="small" disabled={readOnly || task.active || loading || submitting || importing || refreshRequired} onClick={() => setImportOpen(true)}>
          导入 CSV
        </Button>
        <Dropdown
          menu={{
            items: [
              { key: "form", label: formView ? "回到网格" : "表单视图" },
              { key: "json", label: "查看 JSON" },
              { key: "profile", label: "表画像" },
              { key: "ai", label: "AI 填充空值", disabled: readOnly || loading || submitting || importing || refreshRequired },
            ],
            onClick: ({ key }) => {
              if (key === "form") setFormView((value) => !value);
              if (key === "json") {
                const row = rows.find((item) => selected.includes(item.rid)) ?? rows[0];
                if (!row) return;
                const object: Record<string, JsonValue> = {};
                columns.forEach((column, index) => { object[column.name] = row.cells[index] ?? null; });
                setJsonText(JSON.stringify(object, null, 2));
              }
              if (key === "profile") {
                void api.profileTable({ connectionId: tab.connectionId, database: tab.database, schema: tab.schema, table: tab.table })
                  .then((grid) => setJsonText([grid.note, grid.columns.join(" | "), ...grid.rows.map((row) => row.map((cell) => String(cell ?? "")).join(" | "))].join("\n")))
                  .catch((err) => message.error(errorText(err)));
              }
              if (key === "ai") {
                const row = rows[formIndex] ?? rows[0];
                if (!row) return;
                modal.confirm({
                  title: "用 AI 填充空单元格？",
                  content: "会把当前行和表结构发送到你配置的 AI 接口。结果先写进未提交的更改，提交后才落库。",
                  okText: "填充",
                  cancelText: "取消",
                  onOk: () => {
                    const schemaText = columns.map((column) => `${column.name} ${column.dataType}`).join(", ");
                    const current = Object.fromEntries(columns.map((column, index) => [column.name, row.cells[index]]));
                    return api.aiAsk({ task: "fill", sql: JSON.stringify(current), schemaText, prompt: "只补空值", engine: "sqlite" }).then((text) => {
                      const match = text.match(/\{[\s\S]*\}/);
                      if (!match) throw new Error("模型没有返回 JSON 对象");
                      const object = JSON.parse(match[0]) as Record<string, JsonValue>;
                      columns.forEach((column, index) => {
                        if (row.cells[index] == null && object[column.name] !== undefined) updateCell(row.rid, index, object[column.name]);
                      });
                      message.success("已写入未保存的补全，提交后才会落库");
                    });
                  },
                });
              }
            },
          }}
        >
          <Button size="small">更多</Button>
        </Dropdown>
      </div>
      {!importOpen && <TaskProgress task={task} />}
      {dirty && (
        <div className="xb-review-bar">
          <strong>{buildChanges(columns, rows).length} 处待提交</strong>
          <span className="text-xs text-[var(--xb-muted)]">改动先停在这里，提交后才写入数据库。</span>
          <div className="flex-1" />
          <Button size="small" onClick={() => setReviewOpen(true)}>查看</Button>
          <Button size="small" disabled={loading || submitting || importing} onClick={() => void load({ page, size: pageSize, filters, sorts, force: true })}>放弃</Button>
          <Button size="small" type="primary" loading={submitting} disabled={loading || importing || refreshRequired} onClick={() => void commit()}>提交</Button>
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
                    disabled={readOnly || loading || submitting || importing || refreshRequired || row.status === "deleted" || (row.status !== "inserted" && !hasPk)}
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
          disabled={!supportsFilters || loading || submitting || importing}
          style={{ width: 140 }}
          value={draftColumn}
          options={columns.map((column) => ({ value: column.name, label: column.name }))}
          onChange={setDraftColumn}
        />
        <Select size="small" disabled={!supportsFilters || loading || submitting || importing} style={{ width: 120 }} value={draftOp} options={FILTER_OPS} onChange={setDraftOp} />
        {needsValue && (
          <Input
            size="small"
            style={{ width: 180 }}
            placeholder="筛选值"
            disabled={!supportsFilters || loading || submitting || importing}
            value={draftValue}
            onChange={(event) => setDraftValue(event.target.value)}
            onPressEnter={addFilter}
          />
        )}
        <Button size="small" disabled={!supportsFilters || loading || submitting || importing} onClick={addFilter}>
          添加筛选
        </Button>
        {filters.map((filter, index) => (
          <Button key={`${filter.column}-${filter.op}-${index}`} size="small" disabled={loading || submitting || importing} onClick={() => removeFilter(index)}>
            {filter.column} {opLabel(filter.op)} {filter.value ?? ""} ×
          </Button>
        ))}
      </div>
      {!supportsFilters && <Alert type="info" showIcon message="Redis 键浏览暂不支持网格筛选和排序；这些控件已禁用，可在查询编辑器执行 Redis 命令。" />}
      {readOnly && <Alert type="info" showIcon message="这个对象是只读的，不能改数据。" />}
      {!readOnly && !hasPk && columns.length > 0 && (
        <Alert type="warning" showIcon message="没有主键，不能更新或删除已有行。仍可以新增行。" />
      )}
      {error && <Alert type="error" showIcon message={error} />}
      {refreshRequired && <Alert type="warning" showIcon message="数据已提交，请刷新成功后继续编辑。" />}
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
        okButtonProps={{ disabled: refreshRequired }}
        cancelButtonProps={{ disabled: importing }}
        closable={!importing}
        maskClosable={!importing}
        onOk={() => void importCsv()}
        onCancel={() => { if (!importing) setImportOpen(false); }}
      >
        <Space direction="vertical" className="w-full">
          <TaskProgress task={task} />
          <div className="text-sm text-[var(--xb-muted)]">第一行是表头，空单元格保留空字符串，{"\\N"} 表示 NULL。文本本身若是反斜杠加 N，需再加一个反斜杠；普通文件路径保持原样。单次最多 10 万行、50MB。</div>
          <Space.Compact className="w-full">
            <Input value={csvPath} placeholder="选择 CSV 文件" readOnly />
            <Button
              disabled={importing}
              onClick={() => {
                void pickCsvFile().then((path) => {
                  if (path) setCsvPath(path);
                });
              }}
            >
              浏览
            </Button>
          </Space.Compact>
          <Checkbox disabled={importing} checked={truncate} onChange={(event) => setTruncate(event.target.checked)}>
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
