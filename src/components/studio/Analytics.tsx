import { App, Button, Input, InputNumber, Select, Space } from "antd";
import { useEffect, useMemo, useRef, useState } from "react";
import { Area, AreaChart, Bar, BarChart, CartesianGrid, Cell, Legend, Line, LineChart, Pie, PieChart, ResponsiveContainer, Scatter, ScatterChart, Tooltip, XAxis, YAxis } from "recharts";
import { api, errorText, isOperationCancelled, type Dashboard } from "../../api";
import { isNumericType } from "../../format";
import { type JsonValue, type StatementResult } from "../../types";
import { useProfiles, defaultDb } from "./shared";
import { useLongOperation } from "../../operations";
import { TaskProgress } from "../TaskProgress";

const COLORS = ["#6366f1", "#5b8def", "#3cba8e", "#e36d6d", "#b07be0", "#46b5c4"];
const CHARTS = [
  { value: "bar", label: "柱状" },
  { value: "line", label: "折线" },
  { value: "pie", label: "饼图" },
  { value: "area", label: "面积" },
  { value: "scatter", label: "散点" },
];

type Point = { x: string; y: number };
type BoardChart = { id: string; title: string; sql: string; kind: string; page: string; rows: Point[] };

export function BiBoard() {
  const { message } = App.useApp();
  const profiles = useProfiles();
  const [boards, setBoards] = useState<Dashboard[]>([]);
  const [boardId, setBoardId] = useState("");
  const [name, setName] = useState("销售看板");
  const [connectionId, setConnectionId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [sql, setSql] = useState("SELECT country, COUNT(*) FROM authors GROUP BY country");
  const [kind, setKind] = useState("bar");
  const [page, setPage] = useState("总览");
  const [refresh, setRefresh] = useState(0);
  const [rows, setRows] = useState<{ x: string; y: number }[]>([]);
  const [live, setLive] = useState<Record<string, { x: string; y: number }[]>>({});
  const [refreshError, setRefreshError] = useState("");
  const [busy, setBusy] = useState(false);
  const task = useLongOperation();
  const taskRef = useRef(task);
  taskRef.current = task;
  const mounted = useRef(true);
  const flight = useRef<Promise<void> | null>(null);
  const previewFlight = useRef(false);
  const currentSql = useRef({ text: sql, revision: 0 });
  if (currentSql.current.text !== sql) currentSql.current = { text: sql, revision: currentSql.current.revision + 1 };

  useEffect(() => {
    void api.listDashboards().then((items) => { if (mounted.current) setBoards(items); }).catch(() => undefined);
  }, []);

  const board = boards.find((item) => item.id === boardId);
  const charts = useMemo(() => (board?.pages ?? []) as unknown as BoardChart[], [board]);
  const pages = Array.from(new Set(charts.map((chart) => chart.page || "总览")));
  const [activePage, setActivePage] = useState("总览");
  const contextKey = JSON.stringify([boardId, activePage, connectionId, database]);
  const currentContext = useRef({ key: contextKey, revision: 0 });
  if (currentContext.current.key !== contextKey) currentContext.current = { key: contextKey, revision: currentContext.current.revision + 1 };
  const contextRevision = currentContext.current.revision;

  function runCharts(
    requested: Pick<BoardChart, "id" | "title" | "sql">[],
    complete: (data: Record<string, Point[]>, errors: string[], current: () => boolean) => void | Promise<void>,
    previewText?: string,
  ): Promise<void> {
    // The ref closes the gap before React has rendered the disabled controls.
    // A slow refresh never queues another query on the next timer tick.
    if (flight.current || !connectionId) return Promise.resolve();
    const origin = contextRevision;
    const sqlRevision = currentSql.current.revision;
    const originConnection = connectionId;
    const originDatabase = database;
    const current = () => mounted.current && currentContext.current.revision === origin && (previewText === undefined || currentSql.current.revision === sqlRevision);
    previewFlight.current = previewText !== undefined;
    setBusy(true);
    const run = (async () => {
      try {
        await api.connect(originConnection);
        const next: Record<string, Point[]> = {};
        const errors: string[] = [];
        for (const chart of requested) {
          if (!current()) return;
          try {
            const result = await taskRef.current.execute("看板只读查询", (operationId) => api.executeReadQuery({ connectionId: originConnection, database: originDatabase, sql: chart.sql, page: 1, pageSize: 200 }, operationId));
            if (!current()) return;
            const statement = result.statements[0];
            if (!statement?.ok) throw new Error(statement?.error || "查询失败");
            next[chart.id] = chartPoints(statement);
          } catch (error) {
            if (!current() || isOperationCancelled(error)) return;
            errors.push(`${chart.title || "图表"}：${errorText(error)}`);
          }
        }
        if (current()) {
          setRefreshError(errors.join("\n"));
          await complete(next, errors, current);
        }
      } catch (error) {
        if (current() && !isOperationCancelled(error)) setRefreshError(errorText(error));
      }
    })();
    flight.current = run;
    return run.finally(() => {
      if (flight.current === run) flight.current = null;
      previewFlight.current = false;
      if (mounted.current) setBusy(false);
    });
  }

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    void taskRef.current.cancel().catch(() => {});
    setRows([]);
    setLive({});
    setRefreshError("");
  }, [contextKey]);

  useEffect(() => {
    setRows([]);
    if (previewFlight.current) void taskRef.current.cancel().catch(() => {});
  }, [sql]);

  useEffect(() => {
    let disposed = false;
    const origin = contextRevision;
    void (async () => {
      // Let an invalidated query actually stop before loading the new page.
      await flight.current;
      if (disposed || currentContext.current.revision !== origin) return;
      const visible = charts.filter((chart) => (chart.page || "总览") === activePage);
      if (visible.length) await runCharts(visible, (data) => { setLive(data); });
    })();
    return () => { disposed = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [contextKey, charts]);

  function preview() {
    if (!connectionId) { message.error("先选择连接"); return; }
    return runCharts([{ id: "preview", title: "预览", sql }], (data) => { if (data.preview) setRows(data.preview); }, sql);
  }

  useEffect(() => {
    if (!refresh || !connectionId) return;
    const timer = window.setInterval(() => {
      void preview();
    }, refresh * 1000);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh, connectionId, database, sql]);

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Space wrap>
        <Select aria-label="已有看板" style={{ minWidth: 160 }} placeholder="已有看板" value={boardId || undefined} options={boards.map((item) => ({ value: item.id, label: item.name }))} onChange={setBoardId} />
        <Input style={{ width: 160 }} value={name} onChange={(event) => setName(event.target.value)} />
        <Select aria-label="看板连接" style={{ minWidth: 160 }} placeholder="连接" value={connectionId} options={profiles.map((item) => ({ value: item.id, label: item.name }))} onChange={(value) => { setConnectionId(value); setDatabase(defaultDb(profiles, value)); }} />
        <Input aria-label="看板数据库" style={{ width: 120 }} value={database} onChange={(event) => setDatabase(event.target.value)} />
      </Space>
      <Input.TextArea aria-label="看板只读 SQL" rows={3} value={sql} onChange={(event) => setSql(event.target.value)} />
      <Space wrap>
        <Select value={kind} style={{ width: 120 }} options={CHARTS} onChange={setKind} />
        <Input style={{ width: 120 }} value={page} onChange={(event) => setPage(event.target.value)} placeholder="页面" />
        <InputNumber aria-label="自动刷新间隔" min={0} max={3600} value={refresh} onChange={(value) => setRefresh(value ?? 0)} addonAfter="秒刷新" />
        <Button disabled={busy || task.active} onClick={() => void preview()}>预览</Button>
        <Button type="primary" disabled={busy || task.active} onClick={() => {
          if (!connectionId) { message.error("先选择连接"); return; }
          void runCharts([{ id: "preview", title: "预览", sql }], async (points, errors, current) => {
          const data = points.preview;
          if (errors.length || !data || !current()) return;
          const chart = { id: crypto.randomUUID(), title: sql.slice(0, 40), sql, kind, page, rows: data };
          const next: Dashboard = {
            id: boardId,
            name,
            pages: [...charts.filter((item) => item.page !== page || item.sql !== sql), chart] as unknown as JsonValue[],
          };
          const saved = await api.saveDashboard(next);
          if (!current()) return;
          const savedBoards = await api.listDashboards();
          if (!current()) return;
          setBoards(savedBoards);
          setRows(data);
          setBoardId(saved.id);
          message.success("已保存到看板");
        }, sql);
        }}>加入看板</Button>
      </Space>
      <TaskProgress task={task} />
      <div className="text-xs text-[var(--xb-muted)]">仅执行单条 SELECT 或只读 CTE。SQLite、MySQL/MariaDB、PostgreSQL 使用独立只读事务；其他引擎请避免调用有副作用的函数。</div>
      {refreshError && <div className="whitespace-pre-wrap text-xs text-[var(--xb-danger)]">{refreshError}</div>}
      <Chart kind={kind} rows={rows} />
      <Space wrap>
        {pages.map((item) => <Button key={item} type={item === activePage ? "primary" : "default"} onClick={() => setActivePage(item)}>{item}</Button>)}
      </Space>
      {charts.filter((chart) => (chart.page || "总览") === activePage).map((chart) => (
        <div key={chart.id}>
          <div className="text-xs text-[var(--xb-muted)]">{chart.title}</div>
          <Chart kind={chart.kind} rows={live[chart.id] || chart.rows || []} />
        </div>
      ))}
    </div>
  );
}

function chartPoints(statement: StatementResult): { x: string; y: number }[] {
  const yIndex = statement.columns.findIndex((column, index) => index > 0 && isNumericType(column.dataType));
  const valueIndex = yIndex >= 0 ? yIndex : Math.min(1, Math.max(statement.columns.length - 1, 0));
  const labelIndex = valueIndex === 0 ? Math.min(1, statement.columns.length - 1) : 0;
  return statement.rows.map((row) => ({ x: String(row[labelIndex] ?? ""), y: Number(row[valueIndex] ?? 0) }));
}

function Chart({ kind, rows }: { kind: string; rows: { x: string; y: number }[] }) {
  if (!rows.length) return null;
  return (
    <div style={{ width: "100%", height: 240 }}>
      <ResponsiveContainer>
        {kind === "line" ? (
          <LineChart data={rows}><CartesianGrid strokeDasharray="3 3" /><XAxis dataKey="x" /><YAxis /><Tooltip /><Line dataKey="y" stroke="var(--xb-accent)" /></LineChart>
        ) : kind === "area" ? (
          <AreaChart data={rows}><CartesianGrid strokeDasharray="3 3" /><XAxis dataKey="x" /><YAxis /><Tooltip /><Area dataKey="y" stroke="#5b8def" fill="#5b8def" /></AreaChart>
        ) : kind === "pie" ? (
          <PieChart><Pie data={rows} dataKey="y" nameKey="x" outerRadius={80} label>{rows.map((row, index) => <Cell key={row.x} fill={COLORS[index % COLORS.length]} />)}</Pie><Tooltip /><Legend /></PieChart>
        ) : kind === "scatter" ? (
          <ScatterChart><CartesianGrid /><XAxis dataKey="x" /><YAxis dataKey="y" /><Tooltip /><Scatter data={rows} fill="#3cba8e" /></ScatterChart>
        ) : (
          <BarChart data={rows}><CartesianGrid strokeDasharray="3 3" /><XAxis dataKey="x" /><YAxis /><Tooltip /><Bar dataKey="y" fill="var(--xb-accent)" /></BarChart>
        )}
      </ResponsiveContainer>
    </div>
  );
}
