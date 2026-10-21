import { beforeEach, expect, it, vi } from "vitest";

const key = "dataharbor-query-workspace-v1";
beforeEach(() => { localStorage.clear(); vi.resetModules(); });

it("重新加载工作区时恢复未运行的 SQL 和脏状态，不自动打开数据库表", async () => {
  localStorage.setItem(key, JSON.stringify({ version: 1, activeId: "draft", dirty: { draft: true }, tabs: [
    { id: "draft", kind: "query", title: "草稿", connectionId: "saved", database: "A", sql: "SELECT unfinished" },
  ] }));
  const { useTabStore } = await import("../../src/stores/useTabStore");
  expect(useTabStore.getState().tabs).toHaveLength(1);
  expect(useTabStore.getState().tabs[0]).toMatchObject({ sql: "SELECT unfinished" });
  expect(useTabStore.getState().dirty.draft).toBe(true);
  expect(useTabStore.getState().activeId).toBe("draft");
});

it("部分损坏或重复的草稿不会静默过滤后覆盖原记录", async () => {
  const good = { id: "draft", kind: "query", title: "草稿", connectionId: "saved", database: "A", sql: "SELECT valuable" };
  for (const tabs of [[good, { ...good, id: "broken", title: undefined }], [good, good]]) {
    vi.resetModules();
    const raw = JSON.stringify({ version: 1, tabs });
    localStorage.setItem(key, raw);
    const { useTabStore } = await import("../../src/stores/useTabStore");
    expect(useTabStore.getState().draftSaveError).toContain("恢复失败");
    useTabStore.getState().openQuery({ sql: "SELECT new_draft" });
    expect(localStorage.getItem(key)).toBe(raw);
  }
});

it("损坏的恢复资料不会被新工作区静默覆盖", async () => {
  localStorage.setItem(key, "damaged fixture");
  const { useTabStore } = await import("../../src/stores/useTabStore");
  expect(useTabStore.getState().draftSaveError).toContain("恢复失败");
  useTabStore.getState().openQuery({ sql: "SELECT 1" });
  expect(localStorage.getItem(key)).toBe("damaged fixture");
});
