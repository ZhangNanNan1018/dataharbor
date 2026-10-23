import { App } from "antd";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { PanelBoundary } from "../../src/components/PanelBoundary";
import { ExecutionHistory } from "../../src/components/studio/ExecutionHistory";
import { cancelTabOperations } from "../../src/operations";
import { api } from "../../src/api";
import { engineCapabilities } from "../../src/engines";

vi.mock("../../src/operations", async (original) => ({ ...await original<typeof import("../../src/operations")>(), cancelTabOperations: vi.fn() }));
vi.mock("../../src/api", async (original) => {
  const actual = await original<typeof import("../../src/api")>();
  return { ...actual, api: { ...actual.api, listRunHistory: vi.fn() } };
});

beforeEach(() => vi.clearAllMocks());

it("面板异常不影响其他标签，重试等待后台终止", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  let shouldThrow = true;
  function Content() { if (shouldThrow) throw new Error("fixture"); return <div>恢复后的面板</div>; }
  let stopped!: () => void;
  vi.mocked(cancelTabOperations).mockImplementation(() => new Promise((resolve) => { stopped = resolve; }));
  render(<App><div>其他标签仍可用</div><PanelBoundary ownerId="tab"><Content /></PanelBoundary></App>);
  expect(screen.getByText("其他标签仍可用")).toBeInTheDocument();
  expect(screen.getByText("这个面板发生异常")).toBeInTheDocument();
  shouldThrow = false;
  fireEvent.click(screen.getByRole("button", { name: "重新加载面板" }));
  expect(cancelTabOperations).toHaveBeenCalledWith("tab");
  expect(screen.queryByText("恢复后的面板")).not.toBeInTheDocument();
  await act(async () => stopped());
  expect(await screen.findByText("恢复后的面板")).toBeInTheDocument();
});

it("异常退出记录显示待核对状态和已确认提交量", async () => {
  vi.mocked(api.listRunHistory).mockResolvedValue([{ id: "run-id", kind: "scheduled", jobId: "old-job", revision: "revision", status: "unknown", phase: "写入", errorKind: null, startedAt: 100, finishedAt: 105, committed: 500, batches: 1 }]);
  render(<App><ExecutionHistory jobs={[]} /></App>);
  expect(await screen.findByText("有异常退出的任务需要核对结果")).toBeInTheDocument();
  expect(screen.getByText("500 条 / 1 批")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "刷新记录" }));
  await waitFor(() => expect(api.listRunHistory).toHaveBeenCalledTimes(2));
});

it("前端引擎能力由后端生成，保留文档库和 SQL 的差异", () => {
  expect(engineCapabilities("redis")?.filters).toBe(false);
  expect(engineCapabilities("mongodb")?.atomicWrites).toBe(false);
  expect(engineCapabilities("postgres")?.readSnapshot).toBe(true);
  expect(engineCapabilities("snowflake")?.persistentConsole).toBe(false);
});
