import type * as Monaco from "monaco-editor";

export interface SchemaColumn {
  name: string;
  dataType: string;
  primaryKey?: boolean;
}

export interface SchemaTable {
  name: string;
  columns: SchemaColumn[];
}

const KEYWORDS = [
  "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "IS", "NULL", "LIKE", "BETWEEN",
  "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "ON", "AS", "DISTINCT",
  "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "OFFSET", "UNION", "ALL", "INSERT", "INTO",
  "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "VIEW", "INDEX", "ALTER", "DROP",
  "PRIMARY", "KEY", "FOREIGN", "REFERENCES", "CONSTRAINT", "UNIQUE", "DEFAULT", "CASCADE",
  "BEGIN", "COMMIT", "ROLLBACK", "EXPLAIN", "WITH", "CASE", "WHEN", "THEN", "ELSE", "END",
];

const FUNCTIONS = [
  "COUNT", "SUM", "AVG", "MIN", "MAX", "COALESCE", "NULLIF", "CAST", "LOWER", "UPPER",
  "LENGTH", "SUBSTR", "TRIM", "NOW", "CURRENT_TIMESTAMP", "ROUND", "ABS",
];

let tables: SchemaTable[] = [];
let registered = false;

export function setSqlSchema(monaco: typeof Monaco, next: SchemaTable[]) {
  tables = next;
  for (const table of next) {
    const uri = monaco.Uri.parse(`inmemory://schema/${encodeURIComponent(table.name)}.sql`);
    const body = [`-- ${table.name}`, ...table.columns.map((column) => `-- ${column.primaryKey ? "PK " : ""}${column.name} ${column.dataType}`), ""].join("\n");
    const model = monaco.editor.getModel(uri);
    if (model) model.setValue(body);
    else monaco.editor.createModel(body, "sql", uri);
  }
}

export function registerSqlIntelligence(monaco: typeof Monaco) {
  if (registered) return;
  registered = true;
  monaco.languages.registerCompletionItemProvider("sql", {
    triggerCharacters: [".", " "],
    provideCompletionItems(model, position) {
      const word = model.getWordUntilPosition(position);
      const range = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };
      const before = model.getValueInRange({
        startLineNumber: position.lineNumber,
        startColumn: 1,
        endLineNumber: position.lineNumber,
        endColumn: position.column,
      });
      const suggestions: Monaco.languages.CompletionItem[] = [];
      const dot = before.match(/([A-Za-z_][\w$]*)\.\s*[\w$]*$/);
      if (dot) {
        const table = tables.find((item) => item.name.toLowerCase() === dot[1].toLowerCase());
        for (const column of table?.columns ?? []) {
          suggestions.push(item(monaco, column.name, `${column.dataType}${column.primaryKey ? " · 主键" : ""}`, monaco.languages.CompletionItemKind.Field, range));
        }
        return { suggestions };
      }
      const clause = /\b(from|join|update|into|table)\s+[\w$]*$/i.test(before);
      if (clause || before.trim().length === 0) {
        for (const table of tables) {
          suggestions.push(item(monaco, table.name, `${table.columns.length} 列`, monaco.languages.CompletionItemKind.Class, range));
        }
      }
      for (const table of tables) {
        suggestions.push(item(monaco, table.name, "表", monaco.languages.CompletionItemKind.Class, range));
        for (const column of table.columns) {
          suggestions.push(item(monaco, column.name, `${table.name} · ${column.dataType}`, monaco.languages.CompletionItemKind.Field, range));
        }
      }
      for (const keyword of KEYWORDS) {
        suggestions.push(item(monaco, keyword, "关键字", monaco.languages.CompletionItemKind.Keyword, range));
      }
      for (const name of FUNCTIONS) {
        suggestions.push(item(monaco, `${name}()`, "函数", monaco.languages.CompletionItemKind.Function, range, `${name}($0)`));
      }
      return { suggestions };
    },
  });
  monaco.languages.registerHoverProvider("sql", {
    provideHover(model, position) {
      const word = model.getWordAtPosition(position);
      if (!word) return null;
      const table = tables.find((item) => item.name.toLowerCase() === word.word.toLowerCase());
      if (table) {
        return {
          contents: [{ value: `**${table.name}**\n\n${table.columns.map((column) => `\`${column.name}\` ${column.dataType}`).join("\n")}` }],
        };
      }
      const hits = tables.flatMap((item) =>
        item.columns
          .filter((column) => column.name.toLowerCase() === word.word.toLowerCase())
          .map((column) => `${item.name}.${column.name} ${column.dataType}${column.primaryKey ? " 主键" : ""}`),
      );
      if (!hits.length) return null;
      return { contents: [{ value: hits.join("\n") }] };
    },
  });
  monaco.languages.registerDefinitionProvider("sql", {
    provideDefinition(model, position) {
      const word = model.getWordAtPosition(position);
      if (!word) return null;
      const table = tables.find((item) => item.name.toLowerCase() === word.word.toLowerCase());
      if (!table) return null;
      const uri = monaco.Uri.parse(`inmemory://schema/${encodeURIComponent(table.name)}.sql`);
      return { uri, range: { startLineNumber: 1, startColumn: 1, endLineNumber: 1, endColumn: 1 } };
    },
  });
}

export function refreshSqlMarkers(monaco: typeof Monaco, model: Monaco.editor.ITextModel) {
  const text = model.getValue();
  const markers: Monaco.editor.IMarkerData[] = [];
  if (unbalanced(text, "'") || unbalanced(text, '"')) {
    markers.push(marker(monaco, 1, 1, "引号没有成对闭合"));
  }
  const parens = (text.match(/\(/g) ?? []).length - (text.match(/\)/g) ?? []).length;
  if (parens !== 0 && !text.includes("$$")) {
    markers.push(marker(monaco, 1, 1, parens > 0 ? "左括号多于右括号" : "右括号多于左括号"));
  }
  if (tables.length) {
    const known = new Set(tables.map((table) => table.name.toLowerCase()));
    const pattern = /\b(?:from|join|update|into|table)\s+([A-Za-z_][\w$]*)/gi;
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(text))) {
      const name = match[1];
      if (["select", "where", "set", "values"].includes(name.toLowerCase())) continue;
      if (!known.has(name.toLowerCase())) {
        const start = match.index + match[0].lastIndexOf(name) + 1;
        const pos = model.getPositionAt(start - 1);
        markers.push(marker(monaco, pos.lineNumber, pos.column, `当前结构里没有表 ${name}`, name.length));
      }
    }
  }
  monaco.editor.setModelMarkers(model, "dataharbor-sql", markers);
}

function item(
  monaco: typeof Monaco,
  label: string,
  detail: string,
  kind: Monaco.languages.CompletionItemKind,
  range: Monaco.IRange,
  insertText = label,
): Monaco.languages.CompletionItem {
  return {
    label,
    kind,
    detail,
    insertText,
    insertTextRules: insertText.includes("$") ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet : undefined,
    range,
  };
}

function marker(monaco: typeof Monaco, line: number, column: number, message: string, length = 1): Monaco.editor.IMarkerData {
  return {
    severity: monaco.MarkerSeverity.Error,
    message,
    startLineNumber: line,
    startColumn: column,
    endLineNumber: line,
    endColumn: column + length,
  };
}

function unbalanced(text: string, quote: string) {
  let count = 0;
  let escaped = false;
  for (const ch of text) {
    if (escaped) {
      escaped = false;
      continue;
    }
    if (ch === "\\") {
      escaped = true;
      continue;
    }
    if (ch === quote) count += 1;
  }
  return count % 2 === 1;
}
