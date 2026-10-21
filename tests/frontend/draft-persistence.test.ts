import { afterEach, beforeEach, expect, it, vi } from "vitest";

const key = "dataharbor-query-workspace-v1";
beforeEach(() => { localStorage.clear(); vi.resetModules(); vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

it("连续输入合并保存，并在节流窗口结束后保存最后一个字符", async () => {
  const { useTabStore } = await import("../../src/stores/useTabStore");
  const id = useTabStore.getState().openQuery({ sql: "SELECT 0" });
  const writes = vi.spyOn(Storage.prototype, "setItem");
  useTabStore.getState().patchQuery(id, { sql: "SELECT 1" });
  useTabStore.getState().patchQuery(id, { sql: "SELECT 12" });
  useTabStore.getState().patchQuery(id, { sql: "SELECT 123" });
  expect(writes).toHaveBeenCalledTimes(1);
  expect(JSON.parse(localStorage.getItem(key)!).tabs[0].sql).toBe("SELECT 1");
  vi.advanceTimersByTime(400);
  expect(writes).toHaveBeenCalledTimes(2);
  expect(JSON.parse(localStorage.getItem(key)!).tabs[0].sql).toBe("SELECT 123");
});

it.each(["pagehide", "beforeunload", "visibilitychange", "switch"])("%s 会立即补写最后的输入", async (boundary) => {
  const { useTabStore } = await import("../../src/stores/useTabStore");
  const id = useTabStore.getState().openQuery({ sql: "SELECT 0" });
  useTabStore.getState().patchQuery(id, { sql: "SELECT first" });
  useTabStore.getState().patchQuery(id, { sql: "SELECT final" });
  if (boundary === "pagehide" || boundary === "beforeunload") window.dispatchEvent(new Event(boundary));
  else if (boundary === "visibilitychange") {
    const state = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    document.dispatchEvent(new Event("visibilitychange"));
    state.mockRestore();
  } else useTabStore.getState().openQuery();
  expect(JSON.parse(localStorage.getItem(key)!).tabs[0].sql).toBe("SELECT final");
});

it("保存失败仍提示，下一次补写成功后清除提示", async () => {
  const { useTabStore, flushQueryDrafts } = await import("../../src/stores/useTabStore");
  const id = useTabStore.getState().openQuery({ sql: "SELECT 0" });
  const write = vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => { throw new Error("full"); });
  useTabStore.getState().patchQuery(id, { sql: "SELECT keep_me" });
  expect(useTabStore.getState().draftSaveError).toContain("自动保存失败");
  write.mockRestore();
  flushQueryDrafts();
  expect(useTabStore.getState().draftSaveError).toBeNull();
  expect(JSON.parse(localStorage.getItem(key)!).tabs[0].sql).toBe("SELECT keep_me");
});
