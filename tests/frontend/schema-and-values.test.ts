import { describe, expect, it, vi } from "vitest";
import { clearSqlSchema, refreshSqlMarkers, registerSqlIntelligence, setSqlSchema } from "../../src/sqlIntelligence";
import { isNumericType, parseCell } from "../../src/format";
import type { ColumnMeta } from "../../src/types";

function monacoFixture() {
  const models = new Map<string, ReturnType<typeof model>>();
  const providers: Record<string, any> = {};
  function uri(text: string) { return { toString: () => text }; }
  function model(key: string, text = "SELECT * FROM books") {
    return {
      uri: uri(key), getValue: () => text, getWordUntilPosition: () => ({ startColumn: 1, endColumn: 1 }),
      getValueInRange: () => "", getWordAtPosition: () => ({ word: "books" }),
      getPositionAt: (offset: number) => ({ lineNumber: 1, column: offset + 1 }),
      setValue: vi.fn((value: string) => { text = value; }), isDisposed: () => false, dispose: vi.fn(() => models.delete(key)),
    };
  }
  const monaco = {
    Uri: { parse: uri }, MarkerSeverity: { Error: 8, Info: 2 },
    editor: {
      getModel: (key: ReturnType<typeof uri>) => models.get(key.toString()),
      createModel: (text: string, _language: string, key: ReturnType<typeof uri>) => {
        const created = model(key.toString(), text); models.set(key.toString(), created); return created;
      },
      setModelMarkers: vi.fn(),
    },
    languages: {
      CompletionItemKind: { Field: 1, Class: 2, Keyword: 3, Function: 4 }, CompletionItemInsertTextRule: { InsertAsSnippet: 1 },
      registerCompletionItemProvider: (_: string, provider: unknown) => { providers.completion = provider; },
      registerHoverProvider: (_: string, provider: unknown) => { providers.hover = provider; },
      registerDefinitionProvider: (_: string, provider: unknown) => { providers.definition = provider; },
    },
  };
  return { monaco: monaco as any, model, providers, models };
}

describe("多连接 SQL 结构隔离", () => {
  it("同名表的补全、悬停和跳转只使用各自编辑器的结构，清理不影响其他标签", () => {
    const { monaco, model, providers, models } = monacoFixture();
    const a = model("inmemory://queries/a.sql");
    const b = model("inmemory://queries/b.sql");
    registerSqlIntelligence(monaco);
    setSqlSchema(monaco, a as any, [{ name: "books", columns: [{ name: "price", dataType: "NUMERIC" }] }]);
    setSqlSchema(monaco, b as any, [{ name: "books", columns: [{ name: "title", dataType: "TEXT" }] }]);
    const suggestions = (owner: typeof a) => providers.completion.provideCompletionItems(owner, { lineNumber: 1, column: 1 }).suggestions.map((item: { label: string }) => item.label);
    expect(suggestions(a)).toContain("price");
    expect(suggestions(a)).not.toContain("title");
    expect(suggestions(b)).toContain("title");
    expect(providers.hover.provideHover(a, {}).contents[0].value).toContain("NUMERIC");
    const first = providers.definition.provideDefinition(a, {}).uri.toString();
    const second = providers.definition.provideDefinition(b, {}).uri.toString();
    expect(first).not.toBe(second);
    clearSqlSchema(monaco, a as any);
    expect(models.has(first)).toBe(false);
    expect(models.has(second)).toBe(true);
    expect(suggestions(a)).not.toContain("price");
    expect(suggestions(b)).toContain("title");
  });

  it("未加载列的已知表不被误报不存在", () => {
    const { monaco, model } = monacoFixture();
    const owner = model("inmemory://queries/a.sql", "SELECT * FROM books");
    setSqlSchema(monaco, owner as any, [{ name: "books", columns: [], columnsLoaded: false }]);
    refreshSqlMarkers(monaco, owner as any);
    expect(monaco.editor.setModelMarkers).toHaveBeenLastCalledWith(owner, "dataharbor-sql", []);
  });

  it("分批更新目录只改动对应定义，复用其他表模型并回收删除的表", () => {
    const { monaco, model, models } = monacoFixture();
    const owner = model("inmemory://queries/incremental.sql");
    const tables = [
      { name: "books", columns: [], columnsLoaded: false },
      { name: "authors", columns: [], columnsLoaded: false },
    ];
    setSqlSchema(monaco, owner as any, tables);
    const [books, authors] = [...models.values()];
    setSqlSchema(monaco, owner as any, tables);
    expect([...models.values()]).toEqual([books, authors]);
    expect(books.dispose).not.toHaveBeenCalled();
    expect(books.setValue).not.toHaveBeenCalled();
    expect(authors.setValue).not.toHaveBeenCalled();
    const updatedBooks = { name: "books", columns: [{ name: "title", dataType: "TEXT" }], columnsLoaded: true };
    setSqlSchema(monaco, owner as any, [updatedBooks, tables[1]]);
    expect(books.setValue).toHaveBeenCalledTimes(1);
    expect(authors.setValue).not.toHaveBeenCalled();
    setSqlSchema(monaco, owner as any, [updatedBooks]);
    expect(authors.dispose).toHaveBeenCalledTimes(1);
    expect(books.dispose).not.toHaveBeenCalled();
    expect(books.setValue).toHaveBeenCalledTimes(1);
    expect(models.size).toBe(1);
    clearSqlSchema(monaco, owner as any);
  });
});

describe("精确数值编辑", () => {
  const column = (dataType: string): ColumnMeta => ({ name: "value", dataType, nullable: true, primaryKey: false, autoIncrement: false });
  it("高精度十进制和大整数不会经过 JS 浮点数舍入", () => {
    const exact = "123456789012345678.123456789012345678";
    expect(parseCell(exact, column("DECIMAL(38,18)"), false)).toBe(exact);
    expect(parseCell(exact, column("NUMERIC"), false)).toBe(exact);
    expect(isNumericType("fixed")).toBe(true);
    expect(parseCell(exact, column("fixed"), false)).toBe(exact);
    expect(parseCell(" 123.4500 ", column("FIXED"), false)).toBe("123.4500");
    expect(parseCell("9007199254740993", column("BIGINT"), false)).toBe("9007199254740993");
    expect(parseCell("00123", column("VARCHAR(20)"), false)).toBe("00123");
  });
});
