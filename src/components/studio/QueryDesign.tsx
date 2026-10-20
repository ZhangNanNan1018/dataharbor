import { App, Button, Input, Select, Space, Switch, Tag } from "antd";
import { useMemo, useState } from "react";
import { api, errorText } from "../../api";
import { quoteIdent } from "../../engines";
import { useTabStore } from "../../stores/useTabStore";
import { ConnFields, useProfiles } from "./shared";

export function QueryBuilder() {
  const { message } = App.useApp();
  const profiles = useProfiles();
  const openQuery = useTabStore((state) => state.openQuery);
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [schema, setSchema] = useState("");
  const [tables, setTables] = useState<string[]>([]);
  const [picked, setPicked] = useState<{ name: string; alias: string; columns: string[] }[]>([]);
  const [joins, setJoins] = useState<{ left: string; right: string; leftCol: string; rightCol: string; type: string }[]>([]);
  const [wheres, setWheres] = useState<{ column: string; op: string; value: string }[]>([]);
  const [joinDraft, setJoinDraft] = useState({ left: "", right: "", leftCol: "", rightCol: "", type: "INNER" });
  const [whereDraft, setWhereDraft] = useState({ column: "", op: "=", value: "" });
  const profile = profiles.find((item) => item.id === id);
  const engine = profile?.engine;

  async function loadTables(nextId = id, nextDb = database, nextSchema = schema) {
    if (!nextId) return;
    try {
      if (!profiles.find((item) => item.id === nextId)?.connected) await api.connect(nextId);
      const names = await api.listTables(nextId, nextDb, nextSchema || nextDb);
      setTables(names);
    } catch (err) {
      message.error(errorText(err));
    }
  }

  function addTable(name: string) {
    if (!name || picked.some((item) => item.name === name)) return;
    const alias = `t${picked.length + 1}`;
    setPicked((items) => [...items, { name, alias, columns: [] }]);
    if (id) {
      void api.listColumns(id, database, schema || database, name).then((columns) => {
        setPicked((items) =>
          items.map((item) => (item.name === name ? { ...item, columns: columns.map((column) => column.name) } : item)),
        );
      });
    }
  }

  const sql = useMemo(() => {
    if (!picked.length || !engine) return "";
    const cols = picked.flatMap((table) =>
      table.columns.length
        ? table.columns.map((column) => `${table.alias}.${quoteIdent(engine, column)}`)
        : [`${table.alias}.*`],
    );
    let text = `SELECT\n  ${cols.join(",\n  ")}\nFROM ${quoteIdent(engine, picked[0].name)} ${picked[0].alias}`;
    for (const join of joins) {
      const right = picked.find((item) => item.name === join.right);
      const left = picked.find((item) => item.name === join.left);
      if (!right || !left) continue;
      text += `\n${join.type} JOIN ${quoteIdent(engine, right.name)} ${right.alias} ON ${left.alias}.${quoteIdent(engine, join.leftCol)} = ${right.alias}.${quoteIdent(engine, join.rightCol)}`;
    }
    if (wheres.length) {
      text +=
        "\nWHERE " +
        wheres
          .map((item) => `${item.column} ${item.op} '${item.value.replaceAll("'", "''")}'`)
          .join("\n  AND ");
    }
    return text;
  }, [picked, joins, wheres, engine]);

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <ConnFields
        id={id}
        database={database}
        schema={schema}
        onChange={(patch) => {
          const nextId = patch.id ?? id;
          const nextDb = patch.database ?? database;
          const nextSchema = patch.schema ?? schema;
          if (patch.id !== undefined) setId(patch.id);
          if (patch.database !== undefined) setDatabase(patch.database);
          if (patch.schema !== undefined) setSchema(patch.schema);
          if (patch.id || patch.database || patch.schema) void loadTables(nextId, nextDb, nextSchema);
        }}
      />
      <Button onClick={() => void loadTables()}>读取表</Button>
      <div className="flex flex-wrap gap-2" onDragOver={(event) => event.preventDefault()}>
        {tables.map((name) => (
          <Tag
            key={name}
            className="cursor-grab"
            draggable
            onDragStart={(event) => event.dataTransfer.setData("text/plain", name)}
            onClick={() => addTable(name)}
          >
            {name}
          </Tag>
        ))}
      </div>
      <div
        className="min-h-24 rounded border border-dashed border-[var(--xb-line)] p-3"
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => {
          event.preventDefault();
          addTable(event.dataTransfer.getData("text/plain"));
        }}
      >
        {picked.length === 0 && <span className="text-xs text-[var(--xb-muted)]">把表拖到这里，或点击表名。再勾选字段、JOIN 和 WHERE。</span>}
        {picked.map((table) => (
          <div key={table.name} className="mb-2">
            <strong>
              {table.name} {table.alias}
            </strong>
            <div className="text-xs text-[var(--xb-muted)]">{table.columns.join(", ") || "读取列中"}</div>
          </div>
        ))}
      </div>
      <Space wrap>
        <Select style={{ width: 110 }} value={joinDraft.type} options={["INNER", "LEFT", "RIGHT"].map((value) => ({ value, label: value }))} onChange={(type) => setJoinDraft({ ...joinDraft, type })} />
        <Select style={{ width: 120 }} placeholder="左表" value={joinDraft.left || undefined} options={picked.map((item) => ({ value: item.name, label: item.name }))} onChange={(left) => setJoinDraft({ ...joinDraft, left })} />
        <Input style={{ width: 100 }} placeholder="左列" value={joinDraft.leftCol} onChange={(event) => setJoinDraft({ ...joinDraft, leftCol: event.target.value })} />
        <Select style={{ width: 120 }} placeholder="右表" value={joinDraft.right || undefined} options={picked.map((item) => ({ value: item.name, label: item.name }))} onChange={(right) => setJoinDraft({ ...joinDraft, right })} />
        <Input style={{ width: 100 }} placeholder="右列" value={joinDraft.rightCol} onChange={(event) => setJoinDraft({ ...joinDraft, rightCol: event.target.value })} />
        <Button
          onClick={() => {
            if (!joinDraft.left || !joinDraft.right || !joinDraft.leftCol || !joinDraft.rightCol) return;
            setJoins((items) => [...items, joinDraft]);
          }}
        >
          添加 JOIN
        </Button>
      </Space>
      <Space wrap>
        <Input style={{ width: 160 }} placeholder="列，如 t1.id" value={whereDraft.column} onChange={(event) => setWhereDraft({ ...whereDraft, column: event.target.value })} />
        <Select style={{ width: 80 }} value={whereDraft.op} options={["=", "<>", ">", "<", "LIKE"].map((value) => ({ value, label: value }))} onChange={(op) => setWhereDraft({ ...whereDraft, op })} />
        <Input style={{ width: 140 }} placeholder="值" value={whereDraft.value} onChange={(event) => setWhereDraft({ ...whereDraft, value: event.target.value })} />
        <Button onClick={() => whereDraft.column && setWheres((items) => [...items, whereDraft])}>添加 WHERE</Button>
      </Space>
      <Input.TextArea rows={8} value={sql} readOnly />
      <Button
        type="primary"
        disabled={!sql || !id}
        onClick={() => openQuery({ connectionId: id, database, sql, title: "构建的查询" })}
      >
        在编辑器中打开
      </Button>
    </div>
  );
}

export function Designer() {
  const { message } = App.useApp();
  const profiles = useProfiles();
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [schema, setSchema] = useState("");
  const [name, setName] = useState("new_table");
  const [kind, setKind] = useState("table");
  const [body, setBody] = useState("SELECT 1");
  const [columns, setColumns] = useState([{ name: "id", dataType: "INTEGER", nullable: false, primaryKey: true }]);
  const [sql, setSql] = useState("");
  const engine = profiles.find((item) => item.id === id)?.engine ?? "sqlite";

  async function preview() {
    try {
      const text = await api.previewDesign({ engine, database, schema, name, kind, columns, body });
      setSql(text);
    } catch (err) {
      message.error(errorText(err));
    }
  }

  async function run() {
    if (!id || !sql) return;
    try {
      await api.connect(id);
      const result = await api.executeSql({ connectionId: id, database, sql, page: 1, pageSize: 20 });
      const failed = result.statements.find((item) => !item.ok);
      if (failed) message.error(failed.error || "执行失败");
      else message.success("已执行设计 SQL");
    } catch (err) {
      message.error(errorText(err));
    }
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <ConnFields id={id} database={database} schema={schema} onChange={(patch) => {
        if (patch.id !== undefined) setId(patch.id);
        if (patch.database !== undefined) setDatabase(patch.database);
        if (patch.schema !== undefined) setSchema(patch.schema);
      }} />
      <Space wrap>
        <Select value={kind} style={{ width: 140 }} options={[
          { value: "table", label: "表" },
          { value: "view", label: "视图" },
          { value: "index", label: "索引" },
          { value: "procedure", label: "存储过程" },
        ]} onChange={setKind} />
        <Input style={{ width: 180 }} value={name} onChange={(event) => setName(event.target.value)} />
      </Space>
      {kind !== "view" && kind !== "procedure" && columns.map((column, index) => (
        <Space key={index} wrap>
          <Input value={column.name} onChange={(event) => setColumns(columns.map((item, i) => i === index ? { ...item, name: event.target.value } : item))} />
          <Input value={column.dataType} onChange={(event) => setColumns(columns.map((item, i) => i === index ? { ...item, dataType: event.target.value } : item))} />
          <Switch checked={column.nullable} onChange={(nullable) => setColumns(columns.map((item, i) => i === index ? { ...item, nullable } : item))} />
          <span className="text-xs">可空</span>
          <Switch checked={column.primaryKey} onChange={(primaryKey) => setColumns(columns.map((item, i) => i === index ? { ...item, primaryKey } : item))} />
          <span className="text-xs">主键</span>
        </Space>
      ))}
      {kind !== "view" && kind !== "procedure" && (
        <Button onClick={() => setColumns([...columns, { name: "col", dataType: "TEXT", nullable: true, primaryKey: false }])}>加一列</Button>
      )}
      {(kind === "view" || kind === "procedure") && (
        <Input.TextArea rows={6} value={body} onChange={(event) => setBody(event.target.value)} />
      )}
      <Space>
        <Button onClick={() => void preview()}>预览 SQL</Button>
        <Button type="primary" onClick={() => void run()}>执行</Button>
      </Space>
      <Input.TextArea rows={8} value={sql} onChange={(event) => setSql(event.target.value)} />
    </div>
  );
}
