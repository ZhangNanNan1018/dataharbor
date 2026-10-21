import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { App, ConfigProvider } from "antd";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../src/api";
import { BiBoard } from "../../src/components/studio/Analytics";
import type { ExecuteSqlResponse } from "../../src/types";

const operation = vi.hoisted(() => ({
  execute: vi.fn((_label: string, action: (id: string) => Promise<unknown>) => action("bi-operation")),
  cancel: vi.fn(async () => {}),
}));
vi.mock("../../src/operations", () => ({ useLongOperation: () => ({ ...operation, active: false }) }));
vi.mock("../../src/components/TaskProgress", () => ({ TaskProgress: () => null }));
vi.mock("../../src/components/studio/shared", () => ({
  useProfiles: () => [{ id: "bi-connection", name: "BI 测试库", engine: "sqlite", database: "demo" }],
  defaultDb: () => "demo",
}));
vi.mock("../../src/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../src/api")>();
  return { ...actual, api: { ...actual.api, listDashboards: vi.fn(), saveDashboard: vi.fn(), connect: vi.fn(), executeReadQuery: vi.fn(), executeSql: vi.fn() } };
});
// Surface the chart's actual data so tests can detect stale points without
// depending on SVG geometry or ResizeObserver layout in jsdom.
vi.mock("recharts", () => {
  const empty = () => null;
  const chart = ({ data }: { data: unknown }) => <div data-testid="bi-chart">{JSON.stringify(data)}</div>;
  return {
    ResponsiveContainer: ({ children }: { children: React.ReactNode }) => <>{children}</>,
    BarChart: chart, LineChart: chart, PieChart: chart, AreaChart: chart, ScatterChart: chart,
    Area: empty, Bar: empty, CartesianGrid: empty, Cell: empty, Legend: empty, Line: empty,
    Pie: empty, Scatter: empty, Tooltip: empty, XAxis: empty, YAxis: empty,
  };
});

function result(label: string): ExecuteSqlResponse {
  return { stopped: false, statements: [{
    index: 0, sql: "SELECT label, n FROM sample", ok: true, error: null,
    columns: [
      { name: "label", dataType: "TEXT", nullable: true, primaryKey: false, autoIncrement: false },
      { name: "n", dataType: "INTEGER", nullable: true, primaryKey: false, autoIncrement: false },
    ], rows: [[label, 7]], rowsAffected: 0, totalRows: 1, page: 1, pageSize: 200,
    pageable: true, elapsedMs: 1, message: "", truncated: false, hasMore: false,
  }] };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function choose(label: string, text: string) {
  fireEvent.mouseDown(screen.getByRole("combobox", { name: label }));
  const option = await screen.findByText(text);
  fireEvent.click(option);
}

async function mount() {
  const view = render(<ConfigProvider theme={{ token: { motion: false } }}><App><BiBoard /></App></ConfigProvider>);
  await choose("看板连接", "BI 测试库");
  await waitFor(() => expect(screen.getByRole("textbox", { name: "看板数据库" })).toHaveValue("demo"));
  return view;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.listDashboards).mockResolvedValue([]);
  vi.mocked(api.connect).mockResolvedValue({} as never);
  vi.mocked(api.executeReadQuery).mockResolvedValue(result("current"));
});
afterEach(() => { vi.useRealTimers(); });

describe("BI 只读查询生命周期", () => {
  it("预览只调用专用只读入口，快速重复点击不并发", async () => {
    const pending = deferred<ExecuteSqlResponse>();
    vi.mocked(api.executeReadQuery).mockReturnValue(pending.promise);
    await mount();
    fireEvent.click(screen.getByRole("button", { name: /预\s*览/ }));
    fireEvent.click(screen.getByRole("button", { name: /预\s*览/ }));
    await waitFor(() => expect(api.executeReadQuery).toHaveBeenCalledTimes(1));
    expect(api.executeReadQuery).toHaveBeenCalledWith(expect.objectContaining({ connectionId: "bi-connection", database: "demo", page: 1, pageSize: 200 }), "bi-operation");
    expect(api.executeSql).not.toHaveBeenCalled();
    await act(async () => { pending.resolve(result("preview")); });
    expect(screen.getByTestId("bi-chart")).toHaveTextContent("preview");
  });

  it("慢查询跨越多个刷新周期时不排队，完成后下个周期可刷新", async () => {
    const pending = deferred<ExecuteSqlResponse>();
    vi.mocked(api.executeReadQuery).mockReturnValueOnce(pending.promise);
    await mount();
    vi.useFakeTimers();
    fireEvent.change(screen.getByRole("spinbutton", { name: "自动刷新间隔" }), { target: { value: "1" } });
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: /预\s*览/ })); });
    expect(api.executeReadQuery).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(3500); });
    expect(api.executeReadQuery).toHaveBeenCalledTimes(1);
    await act(async () => { pending.resolve(result("first")); });
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(api.executeReadQuery).toHaveBeenCalledTimes(2);
    expect(api.executeSql).not.toHaveBeenCalled();
  });

  it("SQL 改动会取消预览，迟到的旧结果不能覆盖新内容", async () => {
    const pending = deferred<ExecuteSqlResponse>();
    vi.mocked(api.executeReadQuery).mockReturnValueOnce(pending.promise);
    await mount();
    fireEvent.click(screen.getByRole("button", { name: /预\s*览/ }));
    await waitFor(() => expect(api.executeReadQuery).toHaveBeenCalledTimes(1));
    const previousCancels = operation.cancel.mock.calls.length;
    fireEvent.change(screen.getByRole("textbox", { name: "看板只读 SQL" }), { target: { value: "SELECT label, n FROM newer" } });
    expect(operation.cancel.mock.calls.length).toBeGreaterThan(previousCancels);
    await act(async () => { pending.resolve(result("obsolete")); });
    expect(screen.queryByTestId("bi-chart")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /预\s*览/ }));
    await waitFor(() => expect(screen.getByTestId("bi-chart")).toHaveTextContent("current"));
    expect(screen.getByTestId("bi-chart")).not.toHaveTextContent("obsolete");
  });

  it("切换数据库后忽略旧查询错误，自动加载看板也走只读入口", async () => {
    const pending = deferred<ExecuteSqlResponse>();
    vi.mocked(api.executeReadQuery).mockReturnValueOnce(pending.promise);
    vi.mocked(api.listDashboards).mockResolvedValue([{ id: "saved", name: "已存看板", pages: [{ id: "chart", title: "销售图", sql: "SELECT region,n FROM sales", kind: "bar", page: "总览", rows: [] }] }]);
    await mount();
    fireEvent.click(screen.getByRole("button", { name: /预\s*览/ }));
    await waitFor(() => expect(api.executeReadQuery).toHaveBeenCalledTimes(1));
    fireEvent.change(screen.getByRole("textbox", { name: "看板数据库" }), { target: { value: "other" } });
    await act(async () => { pending.reject(new Error("obsolete failure")); });
    expect(screen.queryByText("obsolete failure")).not.toBeInTheDocument();
    await choose("已有看板", "已存看板");
    await waitFor(() => expect(api.executeReadQuery).toHaveBeenCalledTimes(2));
    expect(api.executeReadQuery).toHaveBeenLastCalledWith(expect.objectContaining({ database: "other", sql: "SELECT region,n FROM sales" }), "bi-operation");
    expect(api.executeSql).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByTestId("bi-chart")).toHaveTextContent("current"));
  });
});
