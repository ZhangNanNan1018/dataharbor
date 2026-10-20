import type * as Monaco from "monaco-editor";
import type { ThemeMode } from "./stores/useUiStore";
import { fontMono } from "./theme";

export function applyEditorTheme(monaco: typeof Monaco, mode: ThemeMode) {
  monaco.editor.defineTheme("xb-dark", {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "keyword", foreground: "8B93FF" },
      { token: "string", foreground: "8FBF9F" },
      { token: "number", foreground: "A5B4FC" },
      { token: "comment", foreground: "8B93A1", fontStyle: "italic" },
    ],
    colors: {
      "editor.background": "#1C1F26",
      "editor.foreground": "#E7E9EE",
      "editorLineNumber.foreground": "#6E7684",
      "editor.selectionBackground": "#2E3249",
      "editor.inactiveSelectionBackground": "#292D40",
      "editorCursor.foreground": "#8B93FF",
      "editorBracketMatch.background": "#2E3249",
      "editorBracketMatch.border": "#8B93FF",
      "editorWidget.background": "#252830",
      "editorWidget.border": "#31343D",
    },
  });
  monaco.editor.defineTheme("xb-light", {
    base: "vs",
    inherit: true,
    rules: [
      { token: "keyword", foreground: "4F46E5" },
      { token: "string", foreground: "146C43" },
      { token: "number", foreground: "1D4ED8" },
      { token: "comment", foreground: "8A93A3", fontStyle: "italic" },
    ],
    colors: {
      "editor.background": "#FFFFFF",
      "editor.foreground": "#1C1F26",
      "editorLineNumber.foreground": "#98A2B3",
      "editor.selectionBackground": "#E3E1FB",
      "editor.inactiveSelectionBackground": "#EAE9FC",
      "editorCursor.foreground": "#4F46E5",
      "editorBracketMatch.background": "#EAE9FC",
      "editorBracketMatch.border": "#4F46E5",
      "editorWidget.background": "#FFFFFF",
      "editorWidget.border": "#E4E6EC",
    },
  });
  monaco.editor.setTheme(mode === "dark" ? "xb-dark" : "xb-light");
}

export const editorFont = fontMono;
