import { create } from "zustand";
import { api, errorText } from "../api";
import type { AppInfo, ConnectionPublic } from "../types";

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

export const useConnectionStore = create<ConnectionState>((set) => ({
  profiles: [],
  info: null,
  loading: false,
  error: "",
  load: async () => {
    set({ loading: true, error: "" });
    try {
      const [profiles, info] = await Promise.all([api.listConnections(), api.appInfo()]);
      set({ profiles, info, loading: false });
    } catch (err) {
      set({ loading: false, error: errorText(err) });
    }
  },
  upsert: (profile) =>
    set((state) => {
      const exists = state.profiles.some((item) => item.id === profile.id);
      return {
        profiles: exists
          ? state.profiles.map((item) => (item.id === profile.id ? profile : item))
          : [...state.profiles, profile],
      };
    }),
  remove: (id) => set((state) => ({ profiles: state.profiles.filter((item) => item.id !== id) })),
  markConnected: (id, connected) =>
    set((state) => ({
      profiles: state.profiles.map((item) => (item.id === id ? { ...item, connected } : item)),
    })),
}));
