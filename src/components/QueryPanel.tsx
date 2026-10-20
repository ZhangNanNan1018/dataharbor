import Editor, { type OnMount } from "@monaco-editor/react";
import { App, Alert, Button, Pagination, Select, Space, Table, Tag } from "antd";
import type { ColumnsType } from "antd/es/table";
import { useEffect, useRef, useState } from "react";
import { api, errorText, pickSaveCsv, type PlanNode } from "../api";
import { engineLabel } from "../engines";
import { displayCell, isNumericType } from "../format";
import { applyEditorTheme, editorFont } from "../monacoTheme";
import { refreshSqlMarkers, registerSqlIntelligence, setSqlSchema } from "../sqlIntelligence";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";
import type { ExecuteSqlResponse, JsonValue, QueryTab, StatementResult } from "../types";

interface ResultRow {
  key: string;
  cells: JsonValue[];
}

export function QueryPanel({ tab }: { tab: QueryTab }) {
  const { message } = App.useApp();
  const theme = useUiStore((state) => state.theme);
  const setStatus = useUiStore((state) => state.setStatus);
  const profiles = useConnectionStore((state) => state.profiles);
  const markConnected = useConnectionStore((state) => state.markConnected);
  const patchQuery = useTabStore((state) => state.patchQuery);
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null);
  const runRef = useRef<(mode: "all" | "selection") => Promise<void>>(async () => {});
  const [databases, setDatabases] = useState<string[]>([]);
  const [running, setRunning] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [pageSize, setPageSize] = useState(100);
  const [result, setResult] = useState<ExecuteSqlResponse | null>(null);
  const [active, setActive] = useState(0);
  const [ranSql, setRanSql] = useState("");
  const [plan, setPlan] = useState<PlanNode | null>(null);
  const [resultView, setResultView] = useState<"grid" | "messages" | "plan">("grid");
  const [history, setHistory] = useState<string[]>(() => readHistory());
  const monacoRef = useRef<Parameters<OnMount>[1] | null>(null);
  const pinResult = useUiStore((state) => state.pinResult);

  const statement = result?.statements[active] ?? null;
  const profile = profiles.find((item) => item.id === tab.connectionId);

  useEffect(() => {
    const id = tab.connectionId;
    if (!id) {
      setDatabases([]);
      return;
    }
    const current = useConnectionStore.getState().profiles.find((item) => item.id === id);
    if (!current || current.engine === "sqlite") {
      setDatabases(["main"]);
      return;
    }
    if (!current.connected) {
      setDatabases(current.database ? [current.database] : []);
      return;
    }
    let cancelled = false;
    api
      .listDatabases(id)
      .then((list) => {
        if (!cancelled) setDatabases(list);
      })
      .catch(() => {
        if (!cancelled) setDatabases(current.database ? [current.database] : []);
      });
    return () => {
      cancelled = true;
    };
  }, [tab.connectionId, profiles]);

  async function ensure(id: string) {
    const current = useConnectionStore.getState().profiles.find((item) => item.id === id);
    if (!current) throw new Error("连接不存在");
    if (!current.connected) {
      const connected = await api.connect(id);
      markConnected(id, true);
      if (current.engine !== "sqlite") {
        const list = await api.listDatabases(id).catch(() => [] as string[]);
        if (list.length) setDatabases(list);
      }
      setStatus(`已连接 ${current.name} · ${connected.version}`);
    }
  }

  async function execute(sql: string, page: number, size: number, onlyStatement: number | null) {
    if (!tab.connectionId) throw new Error("先选择连接");
    await ensure(tab.connectionId);
    return api.executeSql({
      connectionId: tab.connectionId,
      database: tab.database,
      sql,
      page,
      pageSize: size,
      onlyStatement,
    });
  }

  async function run(mode: "all" | "selection") {
    if (!tab.connectionId) {
      message.warning("先选择一个连接");
      return;
    }
    const editor = editorRef.current;
    const model = editor?.getModel();
    const selection = editor?.getSelection();
    const selected =
      mode === "selection" && model && selection && !selection.isEmpty()
        ? model.getValueInRange(selection).trim()
        : "";
    if (mode === "selection" && !selected) {
      message.warning("先在编辑器里选中要运行的 SQL");
      return;
    }
    const sql = selected || tab.sql;
    setRunning(true);
    try {
      const response = await execute(sql, 1, pageSize, null);
      setRanSql(sql);
      setResult(response);
      setHistory(pushHistory(sql));
      setActive(0);
      setResultView(response.statements.some((item) => item.columns.length > 0) ? "grid" : "messages");
      const first = response.statements[0];
      setStatus(first?.ok ? first.message : first?.error || "执行结束");
      if (response.stopped) message.error(first?.error || "语句执行失败，后续语句已停止");
    } catch (err) {
      message.error(errorText(err));
    } finally {
      setRunning(false);
    }
  }

  runRef.current = run;

  async function changePage(page: number, size: number) {
    const current = result?.statements[active];
    if (!current || !ranSql) return;
    if (!current.pageable) {
      message.info("这条语句不能分页");
      return;
    }
    setRunning(true);
    try {
      const response = await execute(ranSql, page, size, current.index);
      const updated = response.statements[0];
      if (!updated) return;
      setPageSize(size);
      setResult((prev) => {
        if (!prev) return response;
        return {
          ...prev,
          statements: prev.statements.map((item) => (item.index === updated.index ? updated : item)),
        };
      });
      setStatus(updated.ok ? updated.message : updated.error || "分页失败");
    } catch (err) {
      message.error(errorText(err));
    } finally {
      setRunning(false);
    }
  }

  async function exportCsv() {
    const current = result?.statements[active];
    if (!tab.connectionId || !current?.ok || current.columns.length === 0) {
      message.warning("没有可导出的结果");
      return;
    }
    const path = await pickSaveCsv("query.csv");
    if (!path) return;
    setExporting(true);
    try {
      const exported = await api.exportSqlCsv({
        connectionId: tab.connectionId,
        database: tab.database,
        sql: current.sql,
        path,
      });
      const text = `已导出 ${exported.rowCount} 行${exported.truncated ? "，结果已截断，仅包含部分数据" : ""}`;
      if (exported.truncated) message.warning(text);
      else message.success(text);
      setStatus(`CSV 已写入 ${exported.path}`);
    } catch (err) {
      message.error(errorText(err));
    } finally {
      setExporting(false);
    }
  }

  useEffect(() => {
    const id = tab.connectionId;
    const monaco = monacoRef.current;
    if (!id || !monaco) return;
    let cancelled = false;
    void (async () => {
      try {
        await ensure(id);
        const names = await api.listTables(id, tab.database, profile?.engine === "postgres" ? "public" : "");
        const next = [];
        for (const name of names.slice(0, 40)) {
          const columns = await api.listColumns(id, tab.database, profile?.engine === "postgres" ? "public" : "", name);
          next.push({
            name,
            columns: columns.map((column) => ({
              name: column.name,
              dataType: column.dataType,
              primaryKey: column.primaryKey,
            })),
          });
        }
        if (!cancelled) {
          setSqlSchema(monaco, next);
          const model = editorRef.current?.getModel();
          if (model) refreshSqlMarkers(monaco, model);
        }
      } catch {
        if (!cancelled) setSqlSchema(monaco, []);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tab.connectionId, tab.database, profile?.engine]);

  const onMount: OnMount = (instance, monaco) => {
    editorRef.current = instance;
    monacoRef.current = monaco;
    applyEditorTheme(monaco, useUiStore.getState().theme);
    registerSqlIntelligence(monaco);
    refreshSqlMarkers(monaco, instance.getModel()!);
    instance.onDidChangeModelContent(() => {
      const model = instance.getModel();
      if (model) refreshSqlMarkers(monaco, model);
    });
    instance.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
      void runRef.current("all");
    });
  };

  useEffect(() => {
    const monaco = monacoRef.current;
    if (!monaco) return;
    applyEditorTheme(monaco, theme);
  }, [theme]);

  const columns: ColumnsType<ResultRow> = (statement?.columns ?? []).map((column, index) => ({
    title: column.name,
    key: `${column.name}-${index}`,
    align: isNumericType(column.dataType) ? "right" : "left",
    render: (_value, row) => {
      const cell = row.cells[index];
      return <span className={cell === null ? "xb-null" : isNumericType(column.dataType) ? "xb-num" : undefined}>{displayCell(cell, column.dataType)}</span>;
    },
  }));
  const dataSource: ResultRow[] = (statement?.rows ?? []).map((cells, index) => ({
    key: `${statement?.page ?? 1}-${index}`,
    cells,
  }));

  return (
    <div className="xb-panel">
      <div className="xb-runbar">
        <Select
          style={{ minWidth: 180 }}
          placeholder="选择连接"
          value={tab.connectionId ?? undefined}
          options={profiles.map((item) => ({
            value: item.id,
            label: `${item.name} · ${engineLabel(item.engine)}`,
          }))}
          onChange={(id: string) => {
            const next = profiles.find((item) => item.id === id);
            patchQuery(tab.id, {
              connectionId: id,
              database: next?.engine === "sqlite" ? "main" : next?.database || "",
              title: next ? `查询 · ${next.name}` : tab.title,
            });
            setResult(null);
          }}
        />
        <Select
          style={{ minWidth: 140 }}
          placeholder="数据库"
          value={tab.database || undefined}
          disabled={!tab.connectionId || profile?.engine === "sqlite"}
          options={(databases.length ? databases : tab.database ? [tab.database] : []).map((name) => ({
            value: name,
            label: name,
          }))}
          onChange={(database: string) => patchQuery(tab.id, { database })}
        />
        <Button type="primary" loading={running} onClick={() => void run("all")}>
          运行
        </Button>
        <Button loading={running} onClick={() => void run("selection")}>
          运行选中
        </Button>
        <Button loading={exporting} onClick={() => void exportCsv()}>
          导出 CSV
        </Button>
        <Button
          onClick={() => {
            void api.formatSql(tab.sql, false).then((sql) => patchQuery(tab.id, { sql }));
          }}
        >
          美化
        </Button>
        <Button
          onClick={() => {
            void api.formatSql(tab.sql, true).then((sql) => patchQuery(tab.id, { sql }));
          }}
        >
          压缩
        </Button>
        <Button
          onClick={() => {
            if (!tab.connectionId) return;
            void api
              .explainPlan(tab.connectionId, tab.database, tab.sql)
              .then((node) => {
                setPlan(node);
                setResultView("plan");
                setStatus("已读取执行计划");
              })
              .catch((err) => message.error(errorText(err)));
          }}
        >
          图形解释
        </Button>
        <Select
          style={{ minWidth: 160 }}
          placeholder="历史语句"
          value={undefined}
          options={history.map((sql) => ({ value: sql, label: sql.replaceAll("\n", " ").slice(0, 48) }))}
          onChange={(sql: string) => patchQuery(tab.id, { sql })}
        />
        <Button
          onClick={() => {
            const raw = window.prompt("参数，格式 name=value，多个用逗号", "title=活着");
            if (!raw || !profile) return;
            const params = raw
              .split(",")
              .map((part) => part.split("="))
              .filter((pair) => pair[0])
              .map(([name, ...rest]) => ({ name: name.trim(), value: rest.join("=").trim() }));
            void api
              .bindSql(tab.sql, profile.engine, params)
              .then((sql) => patchQuery(tab.id, { sql }))
              .catch((err) => message.error(errorText(err)));
          }}
        >
          参数
        </Button>
        <span className="text-xs text-[var(--xb-muted)]">Ctrl+Enter 运行全部。Redis 直接写命令，MongoDB 写 find / aggregate / JSON。</span>
      </div>
      <div className="xb-editor">
        <Editor
          language="sql"
          theme={theme === "dark" ? "xb-dark" : "xb-light"}
          value={tab.sql}
          onChange={(value) => patchQuery(tab.id, { sql: value ?? "" })}
          onMount={onMount}
          options={{
            minimap: { enabled: false },
            fontSize: 13,
            fontFamily: editorFont,
            wordWrap: "on",
            automaticLayout: true,
            scrollBeyondLastLine: false,
            tabSize: 2,
          }}
        />
      </div>
      <div className="xb-result-switch" role="tablist">
        <button type="button" className={resultView === "grid" ? "active" : ""} onClick={() => setResultView("grid")}>结果</button>
        <button type="button" className={resultView === "messages" ? "active" : ""} onClick={() => setResultView("messages")}>消息</button>
        <button type="button" className={resultView === "plan" ? "active" : ""} onClick={() => setResultView("plan")}>计划</button>
      </div>
      {resultView === "messages" && (
        <div className="xb-messages">
          {!result && <div>还没有运行。Ctrl/Cmd+Enter 运行全部。</div>}
          {result?.statements.map((item) => (
            <div key={item.index} className={item.ok ? "" : "bad"}>
              语句 {item.index + 1} · {item.elapsedMs} ms · {item.ok ? item.message : item.error || item.message}
            </div>
          ))}
        </div>
      )}
      {resultView === "plan" && (plan ? <PlanTree node={plan} /> : <Alert type="info" showIcon message="点「图形解释」后，执行计划会显示在这里。" />)}
      {resultView === "grid" && statement && !statement.ok && <Alert type="error" showIcon message={statement.error || statement.message} />}
      {resultView === "grid" && statement?.ok && statement.columns.length === 0 && <Alert type="success" showIcon message={statement.message} />}
      {resultView === "grid" && statement?.ok && statement.columns.length > 0 && (
        <>
          <div className="xb-result-meta">
            <Space size={8} wrap>
              {result && result.statements.length > 1 && (
                <Select
                  size="small"
                  value={active}
                  style={{ minWidth: 160 }}
                  options={result.statements.map((item, index) => ({
                    value: index,
                    label: `语句 ${item.index + 1}${item.ok ? "" : " · 失败"}`,
                  }))}
                  onChange={(index: number) => setActive(index)}
                />
              )}
              <span className="text-xs text-[var(--xb-muted)]">
                {statement.message} · {statement.elapsedMs} ms
              </span>
              <Button
                size="small"
                onClick={() => {
                  const text = [statement.columns.map((column) => column.name).join("\t"), ...statement.rows.map((row) => row.map((cell) => String(cell ?? "")).join("\t"))].join("\n");
                  pinResult(tab.title, text);
                  message.success("已固定这份结果");
                }}
              >
                固定结果
              </Button>
              {statement.truncated && <Tag color="gold">已截断</Tag>}
              {!statement.pageable && <Tag>不分页</Tag>}
            </Space>
            <Pagination
              size="small"
              current={statement.page}
              pageSize={statement.pageSize}
              total={totalOf(statement)}
              showSizeChanger
              pageSizeOptions={[50, 100, 200, 500, 1000]}
              disabled={!statement.pageable || running}
              onChange={(page, size) => void changePage(page, size)}
            />
          </div>
          <div className="xb-grid-wrap">
            <Table<ResultRow>
              size="small"
              pagination={false}
              columns={columns}
              dataSource={dataSource}
              sticky
              scroll={{ x: "max-content" }}
            />
          </div>
        </>
      )}
    </div>
  );
}

function readHistory(): string[] {
  try {
    const parsed = JSON.parse(localStorage.getItem("xb-sql-history") || "[]") as unknown;
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string").slice(0, 100) : [];
  } catch {
    return [];
  }
}

function pushHistory(sql: string): string[] {
  const next = [sql, ...readHistory().filter((item) => item !== sql)].slice(0, 100);
  localStorage.setItem("xb-sql-history", JSON.stringify(next));
  return next;
}

function PlanTree({ node }: { node: PlanNode }) {
  const max = maxCost(node) || 1;
  return (
    <div className="overflow-auto rounded border border-[var(--xb-line)] p-2 text-xs">
      <PlanBranch node={node} max={max} />
    </div>
  );
}

function PlanBranch({ node, max }: { node: PlanNode; max: number }) {
  const ratio = Math.min(1, (node.cost ?? 0) / max);
  const tone = ratio > 0.66 ? "var(--xb-danger)" : ratio > 0.33 ? "var(--xb-warning)" : "var(--xb-ok)";
  return (
    <div className="ml-3 border-l border-[var(--xb-line)] pl-2">
      <div className="mb-1 rounded px-2 py-1" style={{ background: `color-mix(in srgb, ${tone} 14%, transparent)`, borderLeft: `3px solid ${tone}` }}>
        <strong>{node.label}</strong>
        {node.detail ? <span className="ml-2 text-[var(--xb-muted)]">{node.detail}</span> : null}
        {node.cost != null ? <span className="ml-2">成本 {node.cost}</span> : null}
        {node.rows != null ? <span className="ml-2">行 {node.rows}</span> : null}
      </div>
      {node.children.map((child, index) => (
        <PlanBranch key={`${child.label}-${index}`} node={child} max={max} />
      ))}
    </div>
  );
}

function maxCost(node: PlanNode): number {
  return Math.max(node.cost ?? 0, ...node.children.map(maxCost));
}

function totalOf(statement: StatementResult): number {
  if (typeof statement.totalRows === "number") return statement.totalRows;
  return (statement.page - 1) * statement.pageSize + statement.rows.length + (statement.hasMore ? 1 : 0);
}
