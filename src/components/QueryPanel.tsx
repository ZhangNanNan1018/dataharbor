import Editor, { type OnMount } from "@monaco-editor/react";
import { App, Alert, Button, Dropdown, Input, Modal, Pagination, Select, Space, Table, Tag } from "antd";
import type { ColumnsType } from "antd/es/table";
import { useEffect, useRef, useState } from "react";
import { api, errorText, isOperationCancelled, pickSaveCsv, type PlanNode } from "../api";
import { useLongOperation } from "../operations";
import { TaskProgress } from "./TaskProgress";
import { engineLabel } from "../engines";
import { displayCell, isNumericType } from "../format";
import { applyEditorTheme, editorFont } from "../monacoTheme";
import { clearSqlSchema, refreshSqlMarkers, registerSqlIntelligence, setSqlSchema } from "../sqlIntelligence";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";
import type { ExecuteSqlResponse, JsonValue, QueryTab, StatementResult } from "../types";

interface ResultRow {
  key: string;
  cells: JsonValue[];
}

interface QueryContext {
  connectionId: string;
  database: string;
  sql: string;
}

interface QueryResult extends ExecuteSqlResponse {
  context: QueryContext;
}

export function QueryPanel({ tab }: { tab: QueryTab }) {
  const { message } = App.useApp();
  const task = useLongOperation(tab.id);
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
  const [storedResult, setResult] = useState<QueryResult | null>(null);
  const [active, setActive] = useState(0);
  const [plan, setPlan] = useState<PlanNode | null>(null);
  const [resultView, setResultView] = useState<"grid" | "messages" | "plan">("grid");
  const [paramOpen, setParamOpen] = useState(false);
  const [paramText, setParamText] = useState("");
  const [history, setHistory] = useState<string[]>(() => readHistory());
  const monacoRef = useRef<Parameters<OnMount>[1] | null>(null);
  const [editorReady, setEditorReady] = useState(0);
  const requestId = useRef(0);
  const toolRequestId = useRef(0);
  const runningRef = useRef<number | null>(null);
  const exportRef = useRef(false);
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const pinResult = useUiStore((state) => state.pinResult);

  const result = storedResult?.context.connectionId === tab.connectionId && storedResult.context.database === tab.database ? storedResult : null;
  const statement = result?.statements[active] ?? null;
  const profile = profiles.find((item) => item.id === tab.connectionId);

  function invalidateResults() {
    requestId.current += 1;
    runningRef.current = null;
    setRunning(false);
    setResult(null);
    setPlan(null);
  }

  useEffect(() => {
    invalidateResults();
    return () => { requestId.current += 1; };
  }, [tab.connectionId, tab.database]);

  function isCurrent(token: number, context: QueryContext) {
    return requestId.current === token && tabRef.current.connectionId === context.connectionId && tabRef.current.database === context.database;
  }

  async function transformDraft(transform: (sql: string) => Promise<string>) {
    const snapshot = { ...tabRef.current };
    const token = requestId.current;
    const toolToken = ++toolRequestId.current;
    const current = () => token === requestId.current && toolToken === toolRequestId.current
      && tabRef.current.connectionId === snapshot.connectionId && tabRef.current.database === snapshot.database
      && tabRef.current.sql === snapshot.sql;
    try {
      const sql = await transform(snapshot.sql);
      if (current()) patchQuery(snapshot.id, { sql });
    } catch (error) {
      if (current()) message.error(errorText(error));
    }
  }

  async function explain() {
    if (!tab.connectionId) return;
    const context = { connectionId: tab.connectionId, database: tab.database, sql: tab.sql };
    const token = requestId.current;
    const toolToken = ++toolRequestId.current;
    const current = () => isCurrent(token, context) && toolToken === toolRequestId.current && tabRef.current.sql === context.sql;
    try {
      await ensure(context.connectionId);
      if (!current()) return;
      const node = await api.explainPlan(context.connectionId, context.database, context.sql);
      if (!current()) return;
      setPlan(node);
      setResultView("plan");
      setStatus("已读取执行计划");
    } catch (error) {
      if (current()) message.error(errorText(error));
    }
  }

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
        if (list.length && tabRef.current.connectionId === id) setDatabases(list);
      }
      setStatus(`已连接 ${current.name} · ${connected.version}`);
    }
  }

  async function execute(context: QueryContext, token: number, page: number, size: number, onlyStatement: number | null) {
    await ensure(context.connectionId);
    if (!isCurrent(token, context)) return null;
    return task.execute(`执行查询 · ${context.database || "默认数据库"}`, (operationId) => api.executeSql({
      ...context,
      page,
      pageSize: size,
      onlyStatement,
    }, operationId));
  }

  async function run(mode: "all" | "selection") {
    if (runningRef.current !== null || task.active || exportRef.current) return;
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
    const context = { connectionId: tab.connectionId, database: tab.database, sql };
    const token = ++requestId.current;
    runningRef.current = token;
    setRunning(true);
    try {
      const response = await execute(context, token, 1, pageSize, null);
      if (!response || !isCurrent(token, context)) return;
      setResult({ ...response, context });
      setHistory(pushHistory(sql));
      setActive(Math.max(0, response.statements.findIndex((item) => item.ok && item.columns.length > 0)));
      setResultView(response.statements.some((item) => item.columns.length > 0) ? "grid" : "messages");
      const first = response.statements[0];
      setStatus(first?.ok ? first.message : first?.error || "执行结束");
      if (response.stopped) message.error(first?.error || "语句执行失败，后续语句已停止");
    } catch (err) {
      if (isCurrent(token, context)) {
        setStatus(errorText(err));
        if (!isOperationCancelled(err)) message.error(errorText(err));
      }
    } finally {
      if (runningRef.current === token) { runningRef.current = null; setRunning(false); }
    }
  }

  runRef.current = run;

  async function changePage(page: number, size: number) {
    const current = result?.statements[active];
    if (!current || !result || runningRef.current !== null || task.active) return;
    if (!current.pageable) {
      message.info("这条语句不能分页");
      return;
    }
    const context = result.context;
    const token = ++requestId.current;
    runningRef.current = token;
    setRunning(true);
    try {
      const response = await execute(context, token, page, size, current.index);
      if (!response || !isCurrent(token, context)) return;
      const updated = response.statements[0];
      if (!updated) return;
      setPageSize(size);
      setResult((prev) => {
        if (!prev) return { ...response, context };
        return {
          ...prev,
          statements: prev.statements.map((item) => (item.index === updated.index ? updated : item)),
        };
      });
      setStatus(updated.ok ? updated.message : updated.error || "分页失败");
    } catch (err) {
      if (isCurrent(token, context)) {
        setStatus(errorText(err));
        if (!isOperationCancelled(err)) message.error(errorText(err));
      }
    } finally {
      if (runningRef.current === token) { runningRef.current = null; setRunning(false); }
    }
  }

  async function exportCsv() {
    const current = result?.statements[active];
    if (exportRef.current || task.active || runningRef.current !== null) return;
    if (!result || !current?.ok || current.columns.length === 0) {
      message.warning("没有可导出的结果");
      return;
    }
    const context = result.context;
    const token = requestId.current;
    exportRef.current = true;
    setExporting(true);
    try {
      const path = await pickSaveCsv("query.csv");
      if (!path) return;
      if (!isCurrent(token, context)) {
        message.warning("查询上下文已改变，请重新运行后导出");
        return;
      }
      const exported = await task.execute("导出查询结果", (operationId) => api.exportSqlCsv({
        connectionId: context.connectionId,
        database: context.database,
        sql: current.sql,
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
      exportRef.current = false;
      setExporting(false);
    }
  }

  useEffect(() => {
    const id = tab.connectionId;
    const monaco = monacoRef.current;
    const model = editorRef.current?.getModel();
    if (!monaco || !model) return;
    clearSqlSchema(monaco, model);
    if (!id) return;
    let cancelled = false;
    void (async () => {
      try {
        await ensure(id);
        const names = await api.listTables(id, tab.database, profile?.engine === "postgres" ? "public" : "");
        const next = names.map((name) => ({ name, columns: [] as { name: string; dataType: string; primaryKey: boolean }[], columnsLoaded: false }));
        for (const name of names.slice(0, 40)) {
          if (cancelled) return;
          const columns = await api.listColumns(id, tab.database, profile?.engine === "postgres" ? "public" : "", name);
          const item = next.find((item) => item.name === name)!;
          item.columns = columns.map((column) => ({
              name: column.name,
              dataType: column.dataType,
              primaryKey: column.primaryKey,
            }));
          item.columnsLoaded = true;
        }
        if (!cancelled) {
          setSqlSchema(monaco, model, next);
          refreshSqlMarkers(monaco, model);
        }
      } catch {
        if (!cancelled) clearSqlSchema(monaco, model);
      }
    })();
    return () => {
      cancelled = true;
      clearSqlSchema(monaco, model);
    };
  }, [tab.connectionId, tab.database, profile?.engine, editorReady]);

  const onMount: OnMount = (instance, monaco) => {
    editorRef.current = instance;
    monacoRef.current = monaco;
    applyEditorTheme(monaco, useUiStore.getState().theme);
    registerSqlIntelligence(monaco);
    setEditorReady((value) => value + 1);
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
          aria-label="查询连接"
          style={{ minWidth: 180 }}
          placeholder="选择连接"
          value={tab.connectionId ?? undefined}
          options={profiles.map((item) => ({
            value: item.id,
            label: `${item.name} · ${engineLabel(item.engine)}`,
          }))}
          onChange={(id: string) => {
            invalidateResults();
            const next = profiles.find((item) => item.id === id);
            patchQuery(tab.id, {
              connectionId: id,
              database: next?.engine === "sqlite" ? "main" : next?.database || "",
              title: next ? `查询 · ${next.name}` : tab.title,
            });
          }}
        />
        <Select
          aria-label="查询数据库"
          style={{ minWidth: 140 }}
          placeholder="数据库"
          value={tab.database || undefined}
          disabled={!tab.connectionId || profile?.engine === "sqlite"}
          options={(databases.length ? databases : tab.database ? [tab.database] : []).map((name) => ({
            value: name,
            label: name,
          }))}
          onChange={(database: string) => { invalidateResults(); patchQuery(tab.id, { database }); }}
        />
        <Button type="primary" disabled={task.active || exporting} loading={running} onClick={() => void run("all")}>
          运行
        </Button>
        <Button disabled={task.active || exporting} loading={running} onClick={() => void run("selection")}>
          运行选中
        </Button>
        <Button disabled={task.active || running} loading={exporting} onClick={() => void exportCsv()}>
          导出 CSV
        </Button>
        <Button
          onClick={() => {
            void transformDraft((sql) => api.formatSql(sql, false));
          }}
        >
          美化
        </Button>
        <Button onClick={() => void explain()}>图形解释</Button>
        <Button onClick={() => { setParamText(""); setParamOpen(true); }}>参数</Button>
        <Dropdown
          menu={{
            items: [
              { key: "minify", label: "压缩" },
              { type: "divider" },
              ...history.map((sql, index) => ({ key: `history-${index}`, label: sql.replaceAll("\n", " ").slice(0, 80) })),
            ],
            onClick: ({ key }) => {
              if (key === "minify") {
                void transformDraft((sql) => api.formatSql(sql, true));
                return;
              }
              const index = Number(String(key).replace("history-", ""));
              const sql = history[index];
              if (sql) patchQuery(tab.id, { sql });
            },
          }}
        >
          <Button>更多</Button>
        </Dropdown>
        <span className="text-xs text-[var(--xb-muted)]">Ctrl+Enter 运行全部。草稿和历史以 SQL 原文保存在本机。</span>
      </div>
      <TaskProgress task={task} />
      <Modal
        title="替换查询参数"
        open={paramOpen}
        okText="应用参数"
        cancelText="取消"
        onCancel={() => setParamOpen(false)}
        onOk={() => {
          if (!profile) {
            message.error("先选择连接");
            return Promise.reject(new Error("missing connection"));
          }
          const params = paramText
            .split(",")
            .map((part) => part.split("="))
            .filter((pair) => pair[0]?.trim())
            .map(([name, ...rest]) => ({ name: name.trim(), value: rest.join("=").trim() }));
          if (!params.length) {
            message.error("格式是 name=value，多个用逗号分隔");
            return Promise.reject(new Error("invalid params"));
          }
          setParamOpen(false);
          void transformDraft((sql) => api.bindSql(sql, profile.engine, params));
        }}
      >
        <Input aria-label="参数" placeholder="name=value，多个用逗号" value={paramText} onChange={(event) => setParamText(event.target.value)} />
      </Modal>
      <div className="xb-editor">
        <Editor
          path={`inmemory://queries/${encodeURIComponent(tab.id)}.sql`}
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
      {result && result.statements.length > 1 && (
        <Select
          aria-label="选择语句结果"
          size="small"
          value={active}
          style={{ width: 200 }}
          options={result.statements.map((item, index) => ({
            value: index,
            label: `语句 ${item.index + 1}${item.ok ? "" : " · 失败"}`,
          }))}
          onChange={(index: number) => { setActive(index); setResultView("grid"); }}
        />
      )}
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
  try { localStorage.setItem("xb-sql-history", JSON.stringify(next)); } catch { /* Query execution must not fail when local storage is full. */ }
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
