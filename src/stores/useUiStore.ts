import { create } from "zustand";

export type ThemeMode = "dark" | "light";
export type ThemePreference = ThemeMode | "system";
export const themeOptions: { key: ThemePreference; label: string }[] = [
  { key: "system", label: "跟随系统" }, { key: "light", label: "明亮" }, { key: "dark", label: "暗色" },
];

interface UiState {
  theme: ThemeMode;
  themePreference: ThemePreference;
  status: string;
  sidebarWidth: number;
  connectionOpen: boolean;
  editingId: string | null;
  initialGroup: string;
  aboutOpen: boolean;
  focusMode: boolean;
  sidebarCollapsed: boolean;
  palette: "command" | "go" | null;
  pinned: { id: string; title: string; text: string }[];
  setTheme: (theme: ThemePreference) => void;
  setStatus: (status: string) => void;
  setSidebarWidth: (width: number) => void;
  openConnection: (id: string | null, group?: string) => void;
  closeConnection: () => void;
  setAboutOpen: (open: boolean) => void;
  toggleFocus: () => void;
  toggleSidebar: () => void;
  openPalette: (mode: "command" | "go") => void;
  closePalette: () => void;
  pinResult: (title: string, text: string) => void;
  unpin: (id: string) => void;
}

function initialTheme(): ThemePreference {
  const stored = localStorage.getItem("xb-theme");
  return stored === "light" || stored === "dark" ? stored : "system";
}

function resolveTheme(preference: ThemePreference): ThemeMode {
  if (preference !== "system") return preference;
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

const initialPreference = initialTheme();

export const useUiStore = create<UiState>((set, get) => ({
  theme: resolveTheme(initialPreference),
  themePreference: initialPreference,
  status: "就绪",
  sidebarWidth: 292,
  connectionOpen: false,
  editingId: null,
  initialGroup: "",
  aboutOpen: false,
  focusMode: false,
  sidebarCollapsed: false,
  palette: null,
  pinned: [],
  setTheme: (preference) => {
    localStorage.setItem("xb-theme", preference);
    set({ themePreference: preference, theme: resolveTheme(preference) });
  },
  setStatus: (status) => set({ status }),
  setSidebarWidth: (width) => set({ sidebarWidth: width }),
  openConnection: (id, group = "") => set({ connectionOpen: true, editingId: id, initialGroup: group }),
  closeConnection: () => set({ connectionOpen: false, editingId: null, initialGroup: "" }),
  setAboutOpen: (open) => set({ aboutOpen: open }),
  toggleFocus: () => set({ focusMode: !get().focusMode }),
  toggleSidebar: () => set({ sidebarCollapsed: !get().sidebarCollapsed }),
  openPalette: (mode) => set({ palette: mode }),
  closePalette: () => set({ palette: null }),
  pinResult: (title, text) =>
    set({
      pinned: [{ id: crypto.randomUUID(), title, text }, ...get().pinned].slice(0, 12),
    }),
  unpin: (id) => set({ pinned: get().pinned.filter((item) => item.id !== id) }),
}));

export function watchSystemTheme() {
  const query = window.matchMedia?.("(prefers-color-scheme: dark)");
  if (!query) return;
  const sync = () => {
    if (useUiStore.getState().themePreference === "system") {
      useUiStore.setState({ theme: query.matches ? "dark" : "light" });
    }
  };
  sync();
  query.addEventListener("change", sync);
  return () => query.removeEventListener("change", sync);
}
