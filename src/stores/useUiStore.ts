import { create } from "zustand";

export type ThemeMode = "dark" | "light";

interface UiState {
  theme: ThemeMode;
  status: string;
  sidebarWidth: number;
  connectionOpen: boolean;
  editingId: string | null;
  aboutOpen: boolean;
  focusMode: boolean;
  sidebarCollapsed: boolean;
  palette: "command" | "go" | null;
  pinned: { id: string; title: string; text: string }[];
  setTheme: (theme: ThemeMode) => void;
  toggleTheme: () => void;
  setStatus: (status: string) => void;
  setSidebarWidth: (width: number) => void;
  openConnection: (id: string | null) => void;
  closeConnection: () => void;
  setAboutOpen: (open: boolean) => void;
  toggleFocus: () => void;
  toggleSidebar: () => void;
  openPalette: (mode: "command" | "go") => void;
  closePalette: () => void;
  pinResult: (title: string, text: string) => void;
  unpin: (id: string) => void;
}

function initialTheme(): ThemeMode {
  const stored = localStorage.getItem("xb-theme");
  return stored === "light" ? "light" : "dark";
}

export const useUiStore = create<UiState>((set, get) => ({
  theme: initialTheme(),
  status: "就绪",
  sidebarWidth: 292,
  connectionOpen: false,
  editingId: null,
  aboutOpen: false,
  focusMode: false,
  sidebarCollapsed: false,
  palette: null,
  pinned: [],
  setTheme: (theme) => {
    localStorage.setItem("xb-theme", theme);
    set({ theme });
  },
  toggleTheme: () => {
    const theme = get().theme === "dark" ? "light" : "dark";
    localStorage.setItem("xb-theme", theme);
    set({ theme });
  },
  setStatus: (status) => set({ status }),
  setSidebarWidth: (width) => set({ sidebarWidth: width }),
  openConnection: (id) => set({ connectionOpen: true, editingId: id }),
  closeConnection: () => set({ connectionOpen: false, editingId: null }),
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
