import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { App, ConfigProvider } from "antd";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { api } from "../../src/api";
import { CellValue } from "../../src/components/CellEditors";
import { ConnFields } from "../../src/components/studio/shared";
import { Sidebar } from "../../src/components/Sidebar";
import { DataGridPanel } from "../../src/components/DataGridPanel";
import { cellEditorText, parseEditedCell } from "../../src/format";
import { invalidateSqlCatalog, watchSqlCatalog } from "../../src/sqlCatalog";
import { ensureConnection } from "../../src/connectionReady";
import { useConnectionStore } from "../../src/stores/useConnectionStore";
import { useTabStore } from "../../src/stores/useTabStore";
import type { ColumnMeta, ConnectionPublic, JsonValue } from "../../src/types";

vi.mock("../../src/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../src/api")>();
  return { ...actual, api: { ...actual.api, connect: vi.fn(), listDatabases: vi.fn(), listSchemas: vi.fn(), listTables: vi.fn(), listViews: vi.fn(), listColumns: vi.fn(), fetchRows: vi.fn(), foreignKeys: vi.fn(), commitChanges: vi.fn() } };
});
const column: ColumnMeta = { name: "payload", dataType: "bson", nullable: true, primaryKey: false, autoIncrement: false };
const fixture: ConnectionPublic = {
  id: "catalog-fixture", name: "目录测试", group: "测试组", engine: "mssql", connected: true,
  host: "localhost", database: "fixture", port: 1433, username: "fixture", filePath: "", color: "",
  hasPassword: false, useSsl: false, sslInsecure: false, sshEnabled: false, sshHost: "", sshPort: 22,
  sshUsername: "", hasSshPassword: false, sshKeyPath: "", httpTunnel: false, httpProxyHost: "", httpProxyPort: 0,
  serverMode: "", authMethod: "", warehouse: "", schemaName: "", addressMap: "", placeholder: false,
};
function mount(node: React.ReactNode) {
  return render(<ConfigProvider theme={{ token: { motion: false } }}><App>{node}</App></ConfigProvider>);
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((finish) => { resolve = finish; });
  return { promise, resolve };
}
beforeEach(() => {
  vi.clearAllMocks();
  invalidateSqlCatalog(fixture.id);
  useConnectionStore.setState({ profiles: [fixture] });
  useTabStore.setState({ tabs: [], activeId: null, dirty: {}, draftSaveError: null });
  vi.mocked(api.listDatabases).mockResolvedValue(["fixture"]);
  vi.mocked(api.listSchemas).mockResolvedValue(["dbo"]);
  vi.mocked(api.listTables).mockResolvedValue(["books"]);
  vi.mocked(api.listViews).mockResolvedValue([]);
  vi.mocked(api.listColumns).mockResolvedValue([column]);
  vi.mocked(api.foreignKeys).mockResolvedValue([]);
  vi.mocked(api.commitChanges).mockResolvedValue({ applied: 1 });
});

describe("结构化单元格编辑", () => {
  it.each<JsonValue>([null, true, false, 3, "text", [1, "two"], { nested: { keep: true } }, { $numberLong: "9007199254740993" }, { $date: { $numberLong: "123" } }])("不修改后失焦保留原类型：%j", (original) => {
    const commit = vi.fn();
    mount(<CellValue column={column} value={original} dirty={false} disabled={false} alwaysEdit connectionId="fixture" database="fixture" onCommit={commit} />);
    const input = screen.getByRole("textbox", { name: "payload 单元格" });
    expect(input).toHaveValue(cellEditorText(original, column));
    fireEvent.blur(input);
    expect(commit).not.toHaveBeenCalled();
  });

  it("校验 JSON 后保留对象，非法 JSON 和不精确整数不写回", () => {
    const commit = vi.fn();
    mount(<CellValue column={column} value={{ keep: true }} dirty={false} disabled={false} alwaysEdit connectionId="fixture" database="fixture" onCommit={commit} />);
    const input = screen.getByRole("textbox", { name: "payload 单元格" });
    fireEvent.change(input, { target: { value: '{"keep":' } });
    fireEvent.blur(input);
    expect(screen.getByRole("alert")).toHaveTextContent("有效 JSON");
    expect(commit).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: '{"keep":false,"items":[1,2]}' } });
    fireEvent.blur(input);
    expect(commit).toHaveBeenCalledWith({ keep: false, items: [1, 2] });
    expect(() => parseEditedCell("9007199254740993", column, null)).toThrow("Extended JSON");
  });

  it("NULL 文本与浮点数在编辑期间不被提前转换", () => {
    expect(parseEditedCell("", { ...column, dataType: "TEXT" }, null)).toBeNull();
    const commit = vi.fn();
    mount(<CellValue column={{ ...column, dataType: "DOUBLE" }} value={1} dirty={false} disabled={false} alwaysEdit connectionId="fixture" database="fixture" onCommit={commit} />);
    const input = screen.getByRole("textbox", { name: "payload 单元格" });
    fireEvent.change(input, { target: { value: "1." } });
    expect(input).toHaveValue("1.");
    expect(commit).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "1.5" } });
    fireEvent.blur(input);
    expect(commit).toHaveBeenCalledWith(1.5);
    expect(cellEditorText('{"keep":true}', { ...column, dataType: "JSONB" })).toBe('{"keep":true}');
    expect(parseEditedCell("false", { ...column, dataType: "JSONB" }, "true")).toBe("false");
  });

  it("点击空值可以清除尚未提交的无效草稿，即使原值已经是 NULL", () => {
    const commit = vi.fn();
    mount(<CellValue column={column} value={null} dirty={false} disabled={false} alwaysEdit connectionId="fixture" database="fixture" onCommit={commit} />);
    const input = screen.getByLabelText("payload 单元格");
    fireEvent.change(input, { target: { value: "invalid-json" } });
    fireEvent.blur(input);
    expect(input).toBeInvalid();
    fireEvent.click(screen.getByText("空"));
    expect(input).toHaveValue("null");
    expect(input).toBeValid();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});

it.each(["valid", "invalid"])("单元格保持焦点时快捷键保存会同步校验最后的输入：%s", async (kind) => {
  vi.mocked(api.fetchRows).mockResolvedValue({
    columns: [{ ...column, name: "id", dataType: "INTEGER", primaryKey: true }, column, { ...column, name: "name", dataType: "TEXT" }],
    rows: [[1, { keep: true }, "original name"]], total: 1, page: 1, pageSize: 100, readOnly: false,
  });
  useTabStore.setState({ activeId: "grid" });
  mount(<DataGridPanel tab={{ id: "grid", kind: "data", title: "grid", connectionId: fixture.id, database: "fixture", schema: "dbo", table: "books", readOnly: false }} />);
  if (kind === "invalid") {
    fireEvent.click(await screen.findByText("original name"));
    const nameInput = screen.getByLabelText("name 单元格");
    fireEvent.change(nameInput, { target: { value: "another pending change" } });
    act(() => nameInput.blur());
  }
  fireEvent.click(await screen.findByText('{"keep":true}'));
  const input = screen.getByLabelText("payload 单元格");
  expect(input).toHaveFocus();
  fireEvent.change(input, { target: { value: kind === "valid" ? '{"keep":false}' : '{"keep":' } });
  fireEvent.keyDown(window, { key: "s", ctrlKey: true });
  if (kind === "valid") {
    await waitFor(() => expect(api.commitChanges).toHaveBeenCalledTimes(1));
    expect(vi.mocked(api.commitChanges).mock.calls[0][0].changes[0].values).toEqual([{ column: "payload", value: { keep: false } }]);
  } else {
    expect(api.commitChanges).not.toHaveBeenCalled();
    expect(input).toBeInvalid();
    expect(input).toHaveFocus();
    expect(await screen.findByRole("alert")).toHaveTextContent("有效 JSON");
  }
});

it("连接参数变更后，旧握手不能把新配置标记为已连接", async () => {
  const oldConnection = deferred<any>();
  const newConnection = deferred<any>();
  vi.mocked(api.connect).mockReturnValueOnce(oldConnection.promise).mockReturnValueOnce(newConnection.promise);
  useConnectionStore.setState({ profiles: [{ ...fixture, connected: false }] });
  const oldRequest = ensureConnection(fixture.id).catch((error) => error);
  useConnectionStore.getState().upsert({ ...fixture, host: "new-host", connected: false });
  const newRequest = ensureConnection(fixture.id);
  oldConnection.resolve({ version: "old" });
  expect(await oldRequest).toBeInstanceOf(Error);
  expect(useConnectionStore.getState().profiles[0].connected).toBe(false);
  newConnection.resolve({ version: "new" });
  await newRequest;
  expect(useConnectionStore.getState().profiles[0]).toMatchObject({ host: "new-host", connected: true });
});

it("断开连接清空补全缓存，不自动对已断开连接发出目录请求", async () => {
  const listener = vi.fn();
  const context = { connectionId: fixture.id, database: "fixture", schema: "dbo" };
  const stop = watchSqlCatalog(context, listener);
  await waitFor(() => expect(listener.mock.lastCall?.[0][0]?.columnsLoaded).toBe(true));
  expect(api.listTables).toHaveBeenCalledTimes(1);
  useConnectionStore.getState().markConnected(fixture.id, false);
  expect(listener.mock.lastCall?.[0]).toEqual([]);
  expect(api.listTables).toHaveBeenCalledTimes(1);
  stop();
  const stopAgain = watchSqlCatalog(context, vi.fn());
  await waitFor(() => expect(api.listTables).toHaveBeenCalledTimes(2));
  stopAgain();
});

it("工作台等待连接成功后才加载目录，保留表选择器", async () => {
  const connection = deferred<any>();
  vi.mocked(api.connect).mockReturnValueOnce(connection.promise);
  useConnectionStore.setState({ profiles: [{ ...fixture, engine: "sqlite", connected: false }] });
  function Fields() {
    const [value, setValue] = useState({ id: fixture.id, database: "main", schema: "main", table: "" });
    return <ConnFields {...value} onChange={(patch) => setValue((current) => ({ ...current, ...patch }))} />;
  }
  mount(<Fields />);
  expect(api.listTables).not.toHaveBeenCalled();
  await act(async () => connection.resolve({ version: "fixture" }));
  await waitFor(() => expect(api.listTables).toHaveBeenCalledWith(fixture.id, "main", "main"));
  expect(screen.getByRole("combobox", { name: "表" })).toBeInTheDocument();
});

it("握手更新连接状态后仍显示迟到的数据库列表", async () => {
  const databases = deferred<string[]>();
  vi.mocked(api.connect).mockResolvedValue({ version: "fixture" });
  vi.mocked(api.listDatabases).mockReturnValue(databases.promise);
  useConnectionStore.setState({ profiles: [{ ...fixture, connected: false }] });
  mount(<ConnFields id={fixture.id} database="" schema="" table="" onChange={vi.fn()} />);
  await waitFor(() => expect(api.listDatabases).toHaveBeenCalled());
  await act(async () => databases.resolve(["late_database"]));
  fireEvent.mouseDown(screen.getByLabelText("数据库", { selector: "input" }));
  expect(await screen.findByText("late_database", { selector: ".ant-select-item-option-content" })).toBeInTheDocument();
});

it("补全共享目录，先发布表名，最多同时读取四张表的列", async () => {
  const names = Array.from({ length: 8 }, (_, index) => `table_${index}`);
  vi.mocked(api.listTables).mockResolvedValueOnce(names);
  const columns = names.map(() => deferred<ColumnMeta[]>());
  vi.mocked(api.listColumns).mockImplementation((_id, _db, _schema, name) => columns[names.indexOf(name)].promise);
  const first = vi.fn();
  const second = vi.fn();
  const context = { connectionId: fixture.id, database: "fixture", schema: "dbo" };
  const stopFirst = watchSqlCatalog(context, first);
  const stopSecond = watchSqlCatalog(context, second);
  await waitFor(() => expect(api.listColumns).toHaveBeenCalledTimes(4));
  expect(api.listTables).toHaveBeenCalledTimes(1);
  expect(first.mock.lastCall?.[0]).toHaveLength(8);
  expect(first.mock.lastCall?.[0].every((table: any) => table.columnsLoaded === false)).toBe(true);
  columns.slice(0, 4).forEach((pending) => pending.resolve([column]));
  await waitFor(() => expect(api.listColumns).toHaveBeenCalledTimes(8));
  columns.slice(4).forEach((pending) => pending.resolve([column]));
  await waitFor(() => expect(second.mock.lastCall?.[0].every((table: any) => table.columnsLoaded)).toBe(true));
  stopFirst(); stopSecond();
  const cached = vi.fn();
  const stopCached = watchSqlCatalog(context, cached);
  expect(cached.mock.lastCall?.[0]).toHaveLength(8);
  expect(api.listTables).toHaveBeenCalledTimes(1);
  stopCached();
});

async function expand(label: string) {
  const row = (await screen.findByText(label)).closest(".ant-tree-treenode")!;
  fireEvent.click(row.querySelector(".ant-tree-switcher")!);
}

it("刷新对象清理整个连接的加载状态和子级缓存，新建查询交由后端分页", async () => {
  vi.mocked(api.listTables).mockResolvedValueOnce(["old_table"]).mockResolvedValue(["new_table"]);
  mount(<Sidebar width={260} />);
  await expand("测试组");
  await expand("目录测试");
  await expand("fixture");
  await expand("dbo");
  await expand("表");
  await screen.findByText("old_table");
  fireEvent.contextMenu(screen.getByText("目录测试"));
  fireEvent.click(await screen.findByText("刷新对象"));
  await expand("目录测试");
  await expand("fixture");
  await expand("dbo");
  await expand("表");
  const table = await screen.findByText("new_table");
  expect(screen.queryByText("old_table")).not.toBeInTheDocument();
  expect(api.listTables).toHaveBeenCalledTimes(2);
  fireEvent.contextMenu(table);
  fireEvent.click(await screen.findByText("新建查询"));
  expect(useTabStore.getState().tabs[0]).toMatchObject({ sql: 'SELECT * FROM [dbo].[new_table];\n' });
});

it("编辑连接后清理树缓存，刷新前迟到的表目录不会重新写回", async () => {
  const oldTables = deferred<string[]>();
  vi.mocked(api.listTables).mockReturnValueOnce(oldTables.promise).mockResolvedValue(["updated_table"]);
  mount(<Sidebar width={260} />);
  await expand("测试组");
  await expand("目录测试");
  await expand("fixture");
  await expand("dbo");
  await expand("表");
  await waitFor(() => expect(api.listTables).toHaveBeenCalledTimes(1));
  act(() => useConnectionStore.getState().upsert({ ...fixture, host: "updated-host" }));
  await act(async () => oldTables.resolve(["stale_table"]));
  expect(screen.queryByText("stale_table")).not.toBeInTheDocument();
  await expand("目录测试");
  await expand("fixture");
  await expand("dbo");
  await expand("表");
  expect(await screen.findByText("updated_table")).toBeInTheDocument();
  expect(screen.queryByText("stale_table")).not.toBeInTheDocument();
  expect(api.listTables).toHaveBeenCalledTimes(2);
});
