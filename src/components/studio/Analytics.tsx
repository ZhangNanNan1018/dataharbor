import { App, Button, Input, InputNumber, Select, Space } from "antd";
import { useEffect, useState } from "react";
import { Area, AreaChart, Bar, BarChart, CartesianGrid, Cell, Legend, Line, LineChart, Pie, PieChart, ResponsiveContainer, Scatter, ScatterChart, Tooltip, XAxis, YAxis } from "recharts";
import { api, errorText, type Dashboard } from "../../api";
import { isNumericType } from "../../format";
import { type JsonValue, type StatementResult } from "../../types";
import { useProfiles, defaultDb } from "./shared";

const COLORS = ["#6366f1", "#5b8def", "#3cba8e", "#e36d6d", "#b07be0", "#46b5c4"];
const CHARTS = [
  { value: "bar", label: "柱状" },
  { value: "line", label: "折线" },
  { value: "pie", label: "饼图" },
  { value: "area", label: "面积" },
  { value: "scatter", label: "散点" },
];

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

  useEffect(() => {
    void api.listDashboards().then(setBoards).catch(() => undefined);
  }, []);

  const board = boards.find((item) => item.id === boardId);
  const charts = (board?.pages ?? []) as unknown as { id: string; title: string; sql: string; kind: string; page: string; rows: { x: string; y: number }[] }[];
  const pages = Array.from(new Set(charts.map((chart) => chart.page || "总览")));
  const [activePage, setActivePage] = useState("总览");

  async function runChart(text: string) {
    if (!connectionId) throw new Error("先选择连接");
    await api.connect(connectionId);
    const result = await api.executeSql({ connectionId, database, sql: text, page: 1, pageSize: 200 });
    const statement = result.statements[0];
    if (!statement?.ok) throw new Error(statement?.error || "查询失败");
    return chartPoints(statement);
  }

  useEffect(() => {
    if (!connectionId) return;
    const visible = charts.filter((chart) => (chart.page || "总览") === activePage);
    if (!visible.length) return;
    let cancelled = false;
    void (async () => {
      const next: Record<string, { x: string; y: number }[]> = {};
      for (const chart of visible) {
        try {
          next[chart.id] = await runChart(chart.sql);
        } catch (err) {
          if (!cancelled) setRefreshError(errorText(err));
        }
      }
      if (!cancelled && Object.keys(next).length) {
        setLive((current) => ({ ...current, ...next }));
        setRefreshError("");
      }
    })();
    return () => {
      cancelled = true;
    };
    // Re-query the open page when the board, page, or connection changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [boardId, activePage, connectionId, database]);

  useEffect(() => {
    if (!refresh || !connectionId) return;
    const timer = window.setInterval(() => {
      void runChart(sql).then(setRows).catch((err) => setRefreshError(errorText(err)));
    }, refresh * 1000);
    return () => window.clearInterval(timer);
  }, [refresh, connectionId, database, sql]);

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Space wrap>
        <Select style={{ minWidth: 160 }} placeholder="已有看板" value={boardId || undefined} options={boards.map((item) => ({ value: item.id, label: item.name }))} onChange={setBoardId} />
        <Input style={{ width: 160 }} value={name} onChange={(event) => setName(event.target.value)} />
        <Select style={{ minWidth: 160 }} placeholder="连接" value={connectionId} options={profiles.map((item) => ({ value: item.id, label: item.name }))} onChange={(value) => { setConnectionId(value); setDatabase(defaultDb(profiles, value)); }} />
        <Input style={{ width: 120 }} value={database} onChange={(event) => setDatabase(event.target.value)} />
      </Space>
      <Input.TextArea rows={3} value={sql} onChange={(event) => setSql(event.target.value)} />
      <Space wrap>
        <Select value={kind} style={{ width: 120 }} options={CHARTS} onChange={setKind} />
        <Input style={{ width: 120 }} value={page} onChange={(event) => setPage(event.target.value)} placeholder="页面" />
        <InputNumber min={0} max={3600} value={refresh} onChange={(value) => setRefresh(value ?? 0)} addonAfter="秒刷新" />
        <Button onClick={() => void runChart(sql).then(setRows).catch((err) => message.error(errorText(err)))}>预览</Button>
        <Button type="primary" onClick={() => void (async () => {
          const data = await runChart(sql);
          const chart = { id: crypto.randomUUID(), title: sql.slice(0, 40), sql, kind, page, rows: data };
          const next: Dashboard = {
            id: boardId,
            name,
            pages: [...charts.filter((item) => item.page !== page || item.sql !== sql), chart] as unknown as JsonValue[],
          };
          const saved = await api.saveDashboard(next);
          setBoardId(saved.id);
          setBoards(await api.listDashboards());
          setRows(data);
          message.success("已保存到看板");
        })().catch((err) => message.error(errorText(err)))}>加入看板</Button>
      </Space>
      {refreshError && <div className="text-xs text-[var(--xb-danger)]">{refreshError}</div>}
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
