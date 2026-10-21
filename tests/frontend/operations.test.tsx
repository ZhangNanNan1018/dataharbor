import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { App, ConfigProvider } from "antd";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, type OperationSnapshot } from "../../src/api";
import { useLongOperation, useOperationStore } from "../../src/operations";
import { TaskProgress } from "../../src/components/TaskProgress";

vi.mock("../../src/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../src/api")>();
  return { ...actual, api: { ...actual.api, createOperation: vi.fn(), operationStatus: vi.fn(), cancelOperation: vi.fn(), releaseOperation: vi.fn() } };
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function snapshot(id: string, patch: Partial<OperationSnapshot> = {}): OperationSnapshot {
  return { id, label: "测试任务", status: "running", phase: "读取源表", completed: 25, total: 100, committed: 0, batches: 0, cancelRequested: false, interruptible: false, message: "正在读取", ...patch };
}

function Harness({ action }: { action: (id: string) => Promise<string> }) {
  const task = useLongOperation("fixture-tab");
  return <><button disabled={task.active} onClick={() => { void task.execute("测试任务", action).catch(() => {}); }}>开始任务</button><TaskProgress task={task} /></>;
}

function mount(action: (id: string) => Promise<string>) {
  return render(<ConfigProvider theme={{ token: { motion: false } }}><App><Harness action={action} /></App></ConfigProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  useOperationStore.setState({ active: [] });
  vi.mocked(api.createOperation).mockResolvedValue(snapshot("op-1", { status: "queued", completed: 0, total: null }));
  vi.mocked(api.operationStatus).mockImplementation(async (id) => snapshot(id));
  vi.mocked(api.cancelOperation).mockImplementation(async (id) => snapshot(id, { cancelRequested: true }));
  vi.mocked(api.releaseOperation).mockResolvedValue(undefined);
});

describe("长任务生命周期", () => {
  it("显示后台测量进度，取消确认后仍等执行返回，并保留已提交批次", async () => {
    const pending = deferred<string>();
    const action = vi.fn(() => pending.promise);
    mount(action);
    fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
    await waitFor(() => expect(action).toHaveBeenCalledWith("op-1"));
    await screen.findByText(/本阶段已处理 25 \/ 100/);
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "25");
    fireEvent.click(screen.getByRole("button", { name: "取消任务" }));
    await waitFor(() => expect(api.cancelOperation).toHaveBeenCalledWith("op-1"));
    await screen.findByText("已请求取消，等待操作停止");
    expect(screen.getByRole("button", { name: "开始任务" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消任务" })).toBeDisabled();
    expect(api.releaseOperation).not.toHaveBeenCalled();
    vi.mocked(api.operationStatus).mockResolvedValue(snapshot("op-1", { status: "cancelled", cancelRequested: true, committed: 500, batches: 1, message: "已提交一批后停止" }));
    await act(async () => pending.reject({ kind: "cancelled", committed: 500, batches: 1, message: "已提交一批后停止" }));
    await screen.findByText("任务已取消");
    expect(screen.getByLabelText("任务进度")).toHaveTextContent("已提交 500 条，1 批");
    expect(screen.getByRole("button", { name: "开始任务" })).toBeEnabled();
    expect(api.releaseOperation).toHaveBeenCalledWith("op-1");
  });

  it("未知总量不显示百分比，取消请求失败可重试且不会假装停止", async () => {
    const pending = deferred<string>();
    vi.mocked(api.operationStatus).mockResolvedValue(snapshot("op-1", { total: null, completed: 40 }));
    vi.mocked(api.cancelOperation).mockRejectedValueOnce(new Error("临时通信失败"));
    mount(() => pending.promise);
    fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
    await screen.findByText(/本阶段已处理 40，总量待确认/);
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "取消任务" }));
    await screen.findByText("取消请求失败：临时通信失败");
    expect(screen.getByRole("button", { name: "取消任务" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "开始任务" })).toBeDisabled();
    vi.mocked(api.operationStatus).mockResolvedValue(snapshot("op-1", { status: "failed", total: null, committed: 20, message: "第二批失败" }));
    await act(async () => pending.reject(new Error("第二批失败")));
    await screen.findByText("任务失败");
    expect(screen.getByLabelText("任务进度")).toHaveTextContent("已提交 20 条");
  });

  it("创建阶段收到取消后使用最终创建的 ID 请求取消，任务执行前不丢取消意图", async () => {
    const created = deferred<OperationSnapshot>();
    vi.mocked(api.createOperation).mockReturnValueOnce(created.promise);
    const action = vi.fn(async () => { throw { kind: "cancelled", message: "开始前已取消", committed: 0, batches: 0 }; });
    mount(action);
    fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
    fireEvent.click(await screen.findByRole("button", { name: "取消任务" }));
    expect(screen.getByText("正在请求取消")).toBeInTheDocument();
    expect(api.cancelOperation).not.toHaveBeenCalled();
    vi.mocked(api.operationStatus).mockResolvedValue(snapshot("created-later", { status: "cancelled", cancelRequested: true }));
    await act(async () => created.resolve(snapshot("created-later", { status: "queued" })));
    await waitFor(() => expect(api.cancelOperation).toHaveBeenCalledWith("created-later"));
    expect(action).toHaveBeenCalledWith("created-later");
    await screen.findByText("任务已取消");
  });

  it("上一任务迟到的轮询不能覆盖新任务进度", async () => {
    const first = deferred<string>();
    const second = deferred<string>();
    const late = deferred<OperationSnapshot>();
    vi.mocked(api.createOperation).mockResolvedValueOnce(snapshot("first", { status: "queued" })).mockResolvedValueOnce(snapshot("second", { status: "queued", phase: "第二任务" }));
    vi.mocked(api.operationStatus).mockReturnValueOnce(late.promise).mockResolvedValueOnce(snapshot("first", { status: "succeeded" })).mockImplementation(async (id) => snapshot(id, { phase: "第二任务", completed: 80 }));
    const action = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    mount(action);
    fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
    await waitFor(() => expect(api.operationStatus).toHaveBeenCalledTimes(1));
    await act(async () => first.resolve("done"));
    await waitFor(() => expect(screen.getByRole("button", { name: "开始任务" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
    await waitFor(() => expect(action).toHaveBeenCalledWith("second"));
    await act(async () => late.resolve(snapshot("first", { phase: "过期阶段", completed: 99 })));
    expect(screen.queryByText("过期阶段")).not.toBeInTheDocument();
    vi.mocked(api.operationStatus).mockResolvedValue(snapshot("second", { status: "succeeded", completed: 100 }));
    await act(async () => second.resolve("done"));
  });
});


it("调用返回失败但后台仍运行时继续保持锁，直到后端确认终态", async () => {
  const pending = deferred<string>();
  mount(() => pending.promise);
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  await waitFor(() => expect(api.createOperation).toHaveBeenCalledTimes(1));
  await act(async () => pending.reject(new Error("响应传输失败")));
  await waitFor(() => expect(api.operationStatus).toHaveBeenCalled());
  expect(screen.getByRole("button", { name: "开始任务" })).toBeDisabled();
  expect(api.releaseOperation).not.toHaveBeenCalled();
  vi.mocked(api.operationStatus).mockResolvedValue(snapshot("op-1", { status: "failed", committed: 500, batches: 1, message: "已写入的第一批保留" }));
  await screen.findByText("任务失败");
  await waitFor(() => expect(screen.getByRole("button", { name: "开始任务" })).toBeEnabled());
  expect(screen.getByLabelText("任务进度")).toHaveTextContent("已知已提交 500 条");
});

it("后台记录不存在时明确要求核对结果，不伪造取消成功", async () => {
  vi.mocked(api.operationStatus).mockRejectedValue({ kind: "operationMissing", message: "任务记录不存在" });
  mount(async () => "returned");
  fireEvent.click(screen.getByRole("button", { name: "开始任务" }));
  await screen.findByText("任务失败");
  expect(screen.getByLabelText("任务进度")).toHaveTextContent("不能仅凭此状态确认成功或回滚");
  expect(screen.queryByText("任务已取消")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "开始任务" })).toBeEnabled();
});


it("已知空任务显示0/0而不是未知总量", () => {
  render(<TaskProgress task={{ active: false, snapshot: snapshot("empty", { status: "succeeded", completed: 0, total: 0 }), cancelError: null, cancelPending: false, cancel: async () => {} }} />);
  expect(screen.getByLabelText("任务进度")).toHaveTextContent("本阶段已处理 0 / 0");
  expect(screen.getByLabelText("任务进度")).not.toHaveTextContent("总量待确认");
});
