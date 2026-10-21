import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { App, ConfigProvider } from "antd";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DataGridPanel } from "../../src/components/DataGridPanel";
import { QueryPanel } from "../../src/components/QueryPanel";
import { Workspace } from "../../src/components/Workspace";
import { AiAssistant } from "../../src/components/studio/AiAssistant";
import { DataSync } from "../../src/components/studio/Synchronization";
import { api, pickSaveCsv } from "../../src/api";
import { useOperationStore } from "../../src/operations";
import { useTabStore } from "../../src/stores/useTabStore";
import { useConnectionStore } from "../../src/stores/useConnectionStore";
import type { ColumnMeta, ConnectionPublic, DataPage, DataTab, QueryTab, StatementResult } from "../../src/types";

vi.mock("../../src/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../src/api")>();
  return { ...actual, api: { ...actual.api,
    fetchRows: vi.fn(), foreignKeys: vi.fn(), commitChanges: vi.fn(),
    listDatabases: vi.fn(), listSchemas: vi.fn(), listTables: vi.fn(), listViews: vi.fn(), connect: vi.fn(),
    executeSql: vi.fn(), exportSqlCsv: vi.fn(), exportTableCsv: vi.fn(),
    aiSettings: vi.fn(), saveAiSettings: vi.fn(),
    createOperation: vi.fn(), operationStatus: vi.fn(), cancelOperation: vi.fn(), releaseOperation: vi.fn(),
    syncRows: vi.fn(), transferRows: vi.fn(),
  }, pickSaveCsv: vi.fn() };
});
// These adapters isolate the panel workflows from Monaco/WebView and specialist cell editors.
vi.mock("@monaco-editor/react", () => ({ default: ({ value, onChange }: { value: string; onChange: (value: string) => void }) =>
  <textarea aria-label="SQL 编辑器" value={value} onChange={(event) => onChange(event.target.value)} />,
}));
vi.mock("../../src/components/CellEditors", () => ({ CellValue: ({ column, value, onCommit, disabled }: {
  column: ColumnMeta; value: unknown; onCommit: (value: string) => void; disabled: boolean;
}) => <input aria-label={column.name} value={String(value ?? "")} disabled={disabled} onChange={(event) => onCommit(event.target.value)} /> }));

const columns: ColumnMeta[] = [
  { name: "id", dataType: "INTEGER", nullable: false, primaryKey: true, autoIncrement: true },
  { name: "name", dataType: "TEXT", nullable: true, primaryKey: false, autoIncrement: false },
];
const profile: ConnectionPublic = {
  id: "connection", name: "测试连接", group: "", color: "", engine: "postgres", host: "localhost", port: 5432,
  username: "fixture", hasPassword: false, database: "A", filePath: "", useSsl: false, sslInsecure: false,
  sshEnabled: false, sshHost: "", sshPort: 22, sshUsername: "", hasSshPassword: false, sshKeyPath: "",
  httpTunnel: false, httpProxyHost: "", httpProxyPort: 0, serverMode: "", authMethod: "", warehouse: "",
  schemaName: "", addressMap: "", connected: true, placeholder: false,
};
function page(table: string): DataPage {
  return { columns, rows: [[1, `${table} original`]], total: 1, page: 1, pageSize: 100, readOnly: false };
}
function grid(id: string): DataTab {
  return { id, kind: "data", title: id, table: id, connectionId: profile.id, database: "A", schema: "public", readOnly: false };
}
function statement(patch: Partial<StatementResult> = {}): StatementResult {
  return { index: 0, sql: "SELECT * FROM books", ok: true, columns, rows: [[1, "A result"]], rowsAffected: 0,
    totalRows: 200, page: 1, pageSize: 100, pageable: true, elapsedMs: 1, message: "读取完成", truncated: false, hasMore: true, ...patch };
}
function mount(node: React.ReactNode) {
  return render(<ConfigProvider theme={{ token: { motion: false } }}><App>{node}</App></ConfigProvider>);
}
function QueryHarness() {
  const tab = useTabStore((state) => state.tabs[0]) as QueryTab;
  return <QueryPanel tab={tab} />;
}
function openQuery() {
  return useTabStore.getState().openQuery({ connectionId: profile.id, database: "A", sql: "SELECT * FROM books" });
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
async function choose(label: string, value: string) {
  fireEvent.mouseDown(screen.getByRole("combobox", { name: label }));
  const options = await screen.findAllByText(value, { selector: ".ant-select-item-option-content" });
  fireEvent.click(options.at(-1)!);
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  useTabStore.setState({ tabs: [], activeId: null, dirty: {}, draftSaveError: null });
  useConnectionStore.setState({ profiles: [profile] });
  useOperationStore.setState({ active: [] });
  const operation = (id: string, status: "queued" | "running" | "succeeded" = "succeeded") => ({ id, label: "测试操作", status, phase: "读取数据", completed: 0, total: null, committed: 0, batches: 0, cancelRequested: false, message: "", interruptible: false });
  vi.mocked(api.createOperation).mockImplementation(async () => operation(crypto.randomUUID(), "queued"));
  vi.mocked(api.operationStatus).mockImplementation(async (id) => operation(id));
  vi.mocked(api.cancelOperation).mockImplementation(async (id) => ({ ...operation(id), cancelRequested: true }));
  vi.mocked(api.releaseOperation).mockResolvedValue(undefined);
  vi.mocked(api.fetchRows).mockImplementation(async (input) => page(input.table));
  vi.mocked(api.foreignKeys).mockResolvedValue([]);
  vi.mocked(api.commitChanges).mockResolvedValue({ applied: 1 });
  vi.mocked(api.listDatabases).mockResolvedValue(["A", "B"]);
  vi.mocked(api.listSchemas).mockResolvedValue(["public"]);
  vi.mocked(api.listTables).mockResolvedValue(["books"]);
  vi.mocked(api.listViews).mockResolvedValue([]);
  vi.mocked(api.connect).mockResolvedValue({ version: "test" });
  vi.mocked(api.executeSql).mockResolvedValue({ statements: [statement()], stopped: false });
  vi.mocked(pickSaveCsv).mockResolvedValue("fixture.csv");
  vi.mocked(api.exportSqlCsv).mockResolvedValue({ path: "fixture.csv", rowCount: 1, truncated: false });
  vi.mocked(api.exportTableCsv).mockResolvedValue({ path: "fixture.csv", rowCount: 1, truncated: false });
});

describe("网格写入边界", () => {
  it("文档库原始快照透传完整字段，不把缺失字段补成 NULL", async () => {
    useConnectionStore.setState({ profiles: [{ ...profile, engine: "mongodb" }] });
    const original = [{ column: "_id", value: "document-id" }, { column: "nested", value: { keep: null } }];
    vi.mocked(api.fetchRows).mockResolvedValueOnce({
      ...page("a"), columns: [{ ...columns[0], name: "_id", dataType: "TEXT" }, columns[1]],
      rows: [["document-id", null]], originalRows: [original],
    });
    useTabStore.setState({ activeId: "a" });
    mount(<DataGridPanel tab={grid("a")} />);
    await screen.findByDisplayValue("document-id");
    fireEvent.change(screen.getByLabelText("name"), { target: { value: "added field" } });
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(api.commitChanges).toHaveBeenCalledTimes(1));
    const change = vi.mocked(api.commitChanges).mock.calls[0][0].changes[0];
    expect(change.original).toEqual(original);
    expect(change.original).not.toContainEqual({ column: "name", value: null });
    expect(change.values).toEqual([{ column: "name", value: "added field" }]);
  });

  it("Redis 明确禁用不支持的筛选和排序", async () => {
    useConnectionStore.setState({ profiles: [{ ...profile, engine: "redis" }] });
    mount(<DataGridPanel tab={grid("a")} />);
    await screen.findByDisplayValue("a original");
    expect(screen.getByRole("button", { name: "添加筛选" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "name" })).toBeDisabled();
    expect(screen.getByPlaceholderText("筛选值")).toBeDisabled();
    expect(screen.getByText(/Redis 键浏览暂不支持网格筛选和排序/)).toBeInTheDocument();
    expect(api.fetchRows).toHaveBeenCalledWith(expect.objectContaining({ filters: [], sorts: [] }));
  });

  it("保存快捷键只提交活动标签，并发送原始行用于冲突检查", async () => {
    useTabStore.setState({ activeId: "a" });
    mount(<><section data-testid="a"><DataGridPanel tab={grid("a")} /></section><section data-testid="b"><DataGridPanel tab={grid("b")} /></section></>);
    await screen.findByDisplayValue("a original");
    await screen.findByDisplayValue("b original");
    fireEvent.change(within(screen.getByTestId("a")).getByLabelText("name"), { target: { value: "a edited" } });
    fireEvent.change(within(screen.getByTestId("b")).getByLabelText("name"), { target: { value: "b edited" } });
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await waitFor(() => expect(api.commitChanges).toHaveBeenCalledTimes(1));
    expect(vi.mocked(api.commitChanges).mock.calls[0][0]).toMatchObject({ table: "a", changes: [{
      kind: "update", values: [{ column: "name", value: "a edited" }], original: [{ column: "id", value: 1 }, { column: "name", value: "a original" }],
    }] });
    expect(within(screen.getByTestId("b")).getByDisplayValue("b edited")).toBeInTheDocument();
  });

  it("请求未完成时阻止重复提交和继续编辑", async () => {
    const pending = deferred<{ applied: number }>();
    vi.mocked(api.commitChanges).mockReturnValueOnce(pending.promise);
    useTabStore.setState({ activeId: "a" });
    mount(<DataGridPanel tab={grid("a")} />);
    const input = await screen.findByDisplayValue("a original");
    fireEvent.change(input, { target: { value: "edited" } });
    fireEvent.click(screen.getAllByRole("button", { name: "提 交" })[0]);
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    fireEvent.keyDown(window, { key: "s", metaKey: true });
    expect(api.commitChanges).toHaveBeenCalledTimes(1);
    expect(input).toBeDisabled();
    await act(async () => pending.resolve({ applied: 1 }));
  });

  it("写入成功但刷新失败后，不会把已提交的草稿再次写入", async () => {
    useTabStore.setState({ activeId: "a" });
    mount(<DataGridPanel tab={grid("a")} />);
    const input = await screen.findByDisplayValue("a original");
    vi.mocked(api.fetchRows).mockRejectedValueOnce(new Error("网络断开"));
    fireEvent.change(input, { target: { value: "edited" } });
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    await screen.findByText("数据已提交，请刷新成功后继续编辑。");
    fireEvent.keyDown(window, { key: "s", ctrlKey: true });
    expect(api.commitChanges).toHaveBeenCalledTimes(1);
    expect(useTabStore.getState().dirty.a).toBe(false);
    expect(screen.getByDisplayValue("edited")).toBeDisabled();
  });

  it("取消放弃修改时不应用新的排序，导出继续使用已显示的条件", async () => {
    mount(<DataGridPanel tab={grid("a")} />);
    const input = await screen.findByDisplayValue("a original");
    fireEvent.change(input, { target: { value: "edited" } });
    fireEvent.click(screen.getByRole("button", { name: "name" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "留 下" }));
    expect(api.fetchRows).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "name" })).toHaveTextContent(/^name$/);
    fireEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    await waitFor(() => expect(api.exportTableCsv).toHaveBeenCalledWith(expect.objectContaining({ sorts: [], filters: [] }), expect.any(String)));
  });
});

describe("查询结果上下文", () => {
  it("换库后清除旧结果，不能把旧 SQL 的导出发往新库", async () => {
    const id = openQuery();
    mount(<QueryHarness />);
    fireEvent.click(screen.getByRole("button", { name: "运 行" }));
    await screen.findByText("A result");
    act(() => useTabStore.getState().patchQuery(id, { database: "B" }));
    expect(screen.queryByText("A result")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    expect(api.exportSqlCsv).not.toHaveBeenCalled();
  });

  it("换库后丢弃迟到的查询响应", async () => {
    const pending = deferred<{ statements: StatementResult[]; stopped: boolean }>();
    vi.mocked(api.executeSql).mockReturnValueOnce(pending.promise);
    const id = openQuery();
    mount(<QueryHarness />);
    fireEvent.click(screen.getByRole("button", { name: "运 行" }));
    await waitFor(() => expect(api.executeSql).toHaveBeenCalledTimes(1));
    act(() => useTabStore.getState().patchQuery(id, { database: "B" }));
    await act(async () => pending.resolve({ statements: [statement()], stopped: false }));
    expect(screen.queryByText("A result")).not.toBeInTheDocument();
  });

  it("保存文件对话框期间换库时取消导出", async () => {
    const picker = deferred<string | null>();
    vi.mocked(pickSaveCsv).mockReturnValueOnce(picker.promise);
    const id = openQuery();
    mount(<QueryHarness />);
    fireEvent.click(screen.getByRole("button", { name: "运 行" }));
    await screen.findByText("A result");
    fireEvent.click(screen.getByRole("button", { name: "导出 CSV" }));
    act(() => useTabStore.getState().patchQuery(id, { database: "B" }));
    await act(async () => picker.resolve("fixture.csv"));
    expect(api.exportSqlCsv).not.toHaveBeenCalled();
  });

  it("UPDATE 后 SELECT 的结果可查看，切回无列语句后选择器仍在", async () => {
    vi.mocked(api.executeSql).mockResolvedValueOnce({ stopped: false, statements: [
      statement({ columns: [], rows: [], sql: "UPDATE books SET name='x'", message: "修改完成" }),
      statement({ index: 1 }),
    ] });
    openQuery();
    mount(<QueryHarness />);
    fireEvent.click(screen.getByRole("button", { name: "运 行" }));
    await screen.findByText("A result");
    await choose("选择语句结果", "语句 1");
    expect(screen.getByText("修改完成")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "选择语句结果" })).toBeInTheDocument();
    await choose("选择语句结果", "语句 2");
    expect(screen.getByText("A result")).toBeInTheDocument();
  });
});

describe("草稿与服务设置", () => {
  it("按需打开工作台工具，切换标签后保留工具草稿", async () => {
    vi.mocked(api.aiSettings).mockResolvedValue({ endpoint: "https://service.example/v1/chat/completions", model: "fixture", hasKey: true });
    const aiTab = useTabStore.getState().openPlaceholder("ai");
    mount(<Workspace />);
    await screen.findByDisplayValue("https://service.example/v1/chat/completions");
    fireEvent.change(screen.getByDisplayValue("这个查询会扫全表吗？"), { target: { value: "保留这个工具草稿" } });
    act(() => { useTabStore.getState().openQuery(); });
    await screen.findByLabelText("SQL 编辑器");
    act(() => useTabStore.getState().setActive(aiTab));
    expect(await screen.findByDisplayValue("保留这个工具草稿")).toBeVisible();
    expect(api.aiSettings).toHaveBeenCalledTimes(1);
  });

  it("未运行的 SQL 会保存恢复记录，关闭前允许保留", async () => {
    const id = openQuery();
    mount(<Workspace />);
    fireEvent.change(screen.getByLabelText("SQL 编辑器"), { target: { value: "SELECT valuable_draft" } });
    expect(useTabStore.getState().dirty[id]).toBe(true);
    expect(JSON.parse(localStorage.getItem("dataharbor-query-workspace-v1")!).tabs[0].sql).toBe("SELECT valuable_draft");
    fireEvent.click(screen.getByLabelText("remove"));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("关闭查询草稿？");
    fireEvent.click(within(dialog).getByRole("button", { name: "留 下" }));
    expect(screen.getByDisplayValue("SELECT valuable_draft")).toBeInTheDocument();
  });

  it("更换 AI 服务后必须提供新密钥，未保存设置不能发送", async () => {
    vi.mocked(api.aiSettings).mockResolvedValue({ endpoint: "https://old.example/v1/chat/completions", model: "fixture", hasKey: true });
    mount(<AiAssistant />);
    const endpoint = await screen.findByDisplayValue("https://old.example/v1/chat/completions");
    fireEvent.change(endpoint, { target: { value: "https://new.example/v1/chat/completions" } });
    fireEvent.click(screen.getByRole("button", { name: "保 存" }));
    expect(api.saveAiSettings).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "发 送" })).toBeDisabled();
  });
});


describe("查询辅助工具的迟到响应", () => {
  it("格式化和参数替换不能覆盖请求期间的新草稿", async () => {
    const formatting = deferred<string>();
    vi.spyOn(api, "formatSql").mockReturnValueOnce(formatting.promise);
    const id = openQuery();
    mount(<QueryHarness />);
    fireEvent.click(screen.getByRole("button", { name: "美 化" }));
    act(() => useTabStore.getState().patchQuery(id, { sql: "SELECT newer_draft" }));
    await act(async () => formatting.resolve("SELECT formatted_old_draft"));
    expect(screen.getByLabelText("SQL 编辑器")).toHaveValue("SELECT newer_draft");
    const binding = deferred<string>();
    vi.spyOn(api, "bindSql").mockReturnValueOnce(binding.promise);
    fireEvent.click(screen.getByRole("button", { name: "参 数" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("参数"), { target: { value: "name=value" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "应用参数" }));
    act(() => useTabStore.getState().patchQuery(id, { sql: "SELECT newest_draft" }));
    await act(async () => binding.resolve("SELECT bound_old_draft"));
    expect(screen.getByLabelText("SQL 编辑器")).toHaveValue("SELECT newest_draft");
  });

  it("换库后不显示旧连接的执行计划", async () => {
    const pending = deferred<{ label: string; detail: string; children: [] }>();
    vi.spyOn(api, "explainPlan").mockReturnValueOnce(pending.promise);
    const id = openQuery();
    mount(<QueryHarness />);
    fireEvent.click(screen.getByRole("button", { name: "图形解释" }));
    await waitFor(() => expect(api.explainPlan).toHaveBeenCalledTimes(1));
    act(() => useTabStore.getState().patchQuery(id, { database: "B" }));
    await act(async () => pending.resolve({ label: "old database plan", detail: "A", children: [] }));
    expect(screen.queryByText("old database plan")).not.toBeInTheDocument();
  });
});


it("文档库部分提交失败后保留草稿但禁止整批重放", async () => {
  useTabStore.setState({ activeId: "a" });
  vi.mocked(api.commitChanges).mockRejectedValueOnce({ kind: "partialWrite", applied: 1, failedIndex: 2, message: "已有部分写入，请核对" });
  mount(<DataGridPanel tab={grid("a")} />);
  fireEvent.change(await screen.findByLabelText("name"), { target: { value: "edited" } });
  fireEvent.keyDown(window, { key: "s", ctrlKey: true });
  await waitFor(() => expect(api.commitChanges).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.getByLabelText("name")).toBeDisabled());
  expect(screen.getByLabelText("name")).toHaveValue("edited");
  fireEvent.keyDown(window, { key: "s", ctrlKey: true });
  expect(api.commitChanges).toHaveBeenCalledTimes(1);
});


it("关闭运行中的查询标签会确认取消，并等后台返回后再移除标签", async () => {
  vi.mocked(api.operationStatus).mockImplementation(async (id) => ({ id, label: "查询", status: "running", phase: "读取", completed: 0, total: null, committed: 0, batches: 0, cancelRequested: false, message: "", interruptible: false }));
  const pending = deferred<{ statements: StatementResult[]; stopped: boolean }>();
  vi.mocked(api.executeSql).mockReturnValueOnce(pending.promise);
  openQuery();
  mount(<Workspace />);
  fireEvent.click(screen.getByRole("button", { name: "运 行" }));
  await waitFor(() => expect(api.executeSql).toHaveBeenCalledTimes(1));
  fireEvent.click(screen.getByLabelText("remove"));
  const dialog = await screen.findByRole("dialog");
  expect(dialog).toHaveTextContent("取消运行中的任务并关闭标签？");
  fireEvent.click(within(dialog).getByRole("button", { name: "取消任务并等待关闭" }));
  await waitFor(() => expect(api.cancelOperation).toHaveBeenCalledTimes(1));
  expect(useTabStore.getState().tabs).toHaveLength(1);
  expect(screen.getByLabelText("SQL 编辑器")).toBeInTheDocument();
  vi.mocked(api.operationStatus).mockImplementation(async (id) => ({ id, label: "查询", status: "cancelled", phase: "读取", completed: 0, total: null, committed: 0, batches: 0, cancelRequested: true, message: "查询已停止", interruptible: false }));
  await act(async () => pending.reject({ kind: "cancelled", message: "查询已停止", committed: 0, batches: 0 }));
  await waitFor(() => expect(useTabStore.getState().tabs).toHaveLength(0));
});


it("同步界面把操作ID传给后台，取消中锁住目标并保留部分提交说明", async () => {
  useConnectionStore.setState({ profiles: [{ ...profile, id: "source", name: "源连接" }, { ...profile, id: "target", name: "目标连接" }] });
  const pending = deferred<{ inserted: number; updated: number; deleted: number; truncated: boolean; note: string }>();
  vi.mocked(api.listTables).mockResolvedValue(["source_rows", "target_rows"]);
  vi.mocked(api.syncRows).mockReturnValueOnce(pending.promise);
  const operation = { id: "sync-task", label: "数据同步", status: "running" as const, phase: "写入目标", completed: 500, total: 1000, committed: 500, batches: 1, cancelRequested: false, interruptible: false, message: "已提交第一批" };
  vi.mocked(api.createOperation).mockResolvedValueOnce({ ...operation, status: "queued", completed: 0, committed: 0, batches: 0 });
  vi.mocked(api.operationStatus).mockResolvedValue(operation);
  vi.mocked(api.cancelOperation).mockResolvedValueOnce({ ...operation, cancelRequested: true });
  mount(<DataSync />);
  await choose("源连接", "源连接 · PostgreSQL");
  await choose("目标连接", "目标连接 · PostgreSQL");
  await waitFor(() => expect(api.listTables).toHaveBeenCalled());
  await choose("源表", "source_rows");
  await choose("目标表", "target_rows");
  fireEvent.click(screen.getByRole("button", { name: "按主键同步" }));
  await waitFor(() => expect(api.syncRows).toHaveBeenCalledWith(expect.objectContaining({ sourceId: "source", targetId: "target", sourceTable: "source_rows", targetTable: "target_rows" }), "sync-task"));
  await screen.findByText(/已提交 500 条，1 批/);
  fireEvent.click(screen.getByRole("button", { name: "取消任务" }));
  await waitFor(() => expect(api.cancelOperation).toHaveBeenCalledWith("sync-task"));
  expect(screen.getByRole("button", { name: /按主键同步/ })).toBeDisabled();
  expect(screen.getByRole("combobox", { name: "目标表" })).toBeDisabled();
  vi.mocked(api.operationStatus).mockResolvedValue({ ...operation, status: "cancelled", cancelRequested: true, message: "已完成的第一批保留" });
  await act(async () => pending.reject({ kind: "cancelled", message: "已完成的第一批保留", committed: 500, batches: 1 }));
  await screen.findByText("任务已取消");
  expect(screen.getByLabelText("任务进度")).toHaveTextContent("已完成的第一批保留");
  expect(screen.queryByText(/插入 500，更新/)).not.toBeInTheDocument();
});
