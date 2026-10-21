import { create } from "zustand";
import { api, errorText } from "../api";
import type { AppInfo, ConnectionPublic } from "../types";
import { invalidateSqlCatalog } from "../sqlCatalog";

interface ConnectionState {
  profiles: ConnectionPublic[];
  info: AppInfo | null;
  loading: boolean;
  error: string;
  load: () => Promise<void>;
  upsert: (profile: ConnectionPublic) => void;
  remove: (id: string) => void;
  markConnected: (id: string, connected: boolean) => void;
}

export const useConnectionStore = create<ConnectionState>((set, get) => ({
  profiles: [],
  info: null,
  loading: false,
  error: "",
  load: async () => {
    set({ loading: true, error: "" });
    try {
      const [profiles, info] = await Promise.all([api.listConnections(), api.appInfo()]);
      get().profiles.forEach((profile) => invalidateSqlCatalog(profile.id, false));
      set({ profiles, info, loading: false });
    } catch (err) {
      set({ loading: false, error: errorText(err) });
    }
  },
  upsert: (profile) => {
    invalidateSqlCatalog(profile.id, false);
    set((state) => {
      const exists = state.profiles.some((item) => item.id === profile.id);
      return {
        profiles: exists
          ? state.profiles.map((item) => (item.id === profile.id ? profile : item))
          : [...state.profiles, profile],
      };
    });
  },
  remove: (id) => {
    invalidateSqlCatalog(id, false);
    set((state) => ({ profiles: state.profiles.filter((item) => item.id !== id) }));
  },
  markConnected: (id, connected) => {
    if (!connected) invalidateSqlCatalog(id, false);
    set((state) => ({
      profiles: state.profiles.map((item) => (item.id === id && item.connected !== connected ? { ...item, connected } : item)),
    }));
  },
}));
