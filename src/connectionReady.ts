import { api } from "./api";
import { useConnectionStore } from "./stores/useConnectionStore";
import type { ConnectionPublic } from "./types";

const pending = new Map<string, { profile: ConnectionPublic; promise: Promise<void> }>();

/** Concurrent panels share one handshake before requesting this connection's catalog. */
export function ensureConnection(id: string): Promise<void> {
  const profile = useConnectionStore.getState().profiles.find((item) => item.id === id);
  if (!profile) return Promise.reject(new Error("连接不存在"));
  if (profile.connected) return Promise.resolve();
  const existing = pending.get(id);
  if (existing?.profile === profile) return existing.promise;
  const request = api.connect(id).then(() => {
    if (useConnectionStore.getState().profiles.find((item) => item.id === id) !== profile) {
      throw new Error("连接资料已变化，请重新连接");
    }
    useConnectionStore.getState().markConnected(id, true);
  }).finally(() => {
    if (pending.get(id)?.promise === request) pending.delete(id);
  });
  pending.set(id, { profile, promise: request });
  return request;
}
