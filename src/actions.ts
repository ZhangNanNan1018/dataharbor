import { api } from "./api";
import { useConnectionStore } from "./stores/useConnectionStore";
import { useTabStore } from "./stores/useTabStore";
import { useUiStore } from "./stores/useUiStore";

export async function openSampleBookstore(): Promise<void> {
  const profile = await api.createSample();
  useConnectionStore.getState().upsert(profile);
  const connected = await api.connect(profile.id);
  useConnectionStore.getState().markConnected(profile.id, true);
  useTabStore.getState().openData({
    connectionId: profile.id,
    database: "main",
    schema: "main",
    table: "books",
    readOnly: false,
    title: "books",
  });
  useUiStore.getState().setStatus(`已打开示例书店 · SQLite ${connected.version}`);
}
