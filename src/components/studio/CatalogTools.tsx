import { App, Button, Input, InputNumber, Select, Space } from "antd";
import { useState } from "react";
import { api, errorText, type GridResult } from "../../api";
import { useTabStore } from "../../stores/useTabStore";
import { ConnFields, Grid, useProfiles, defaultDb } from "./shared";

export function GenerateMask() {
  const { message } = App.useApp();
  const [ref, setRef] = useState({ id: "", database: "", schema: "", table: "" });
  const [count, setCount] = useState(20);
  const [columns, setColumns] = useState("name,email");
  const [doc, setDoc] = useState("");
  const [analysis, setAnalysis] = useState<GridResult | null>(null);
  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <ConnFields id={ref.id} database={ref.database} schema={ref.schema} table={ref.table} onChange={(patch) => setRef({ ...ref, ...patch })} />
      <Space wrap>
        <InputNumber min={1} max={500} value={count} onChange={(value) => setCount(value ?? 1)} />
        <Button onClick={() => void api.generateRows({ connectionId: ref.id, database: ref.database, schema: ref.schema, table: ref.table, count }).then((applied) => message.success(`生成 ${applied} 行`)).catch((err) => message.error(errorText(err)))}>生成数据</Button>
        <Input style={{ width: 220 }} value={columns} onChange={(event) => setColumns(event.target.value)} placeholder="要脱敏的列，逗号分隔" />
        <Button onClick={() => void api.maskColumns({ connectionId: ref.id, database: ref.database, schema: ref.schema, table: ref.table, columns: columns.split(",").map((item) => item.trim()).filter(Boolean) }).then((text) => message.success(text)).catch((err) => message.error(errorText(err)))}>脱敏</Button>
      </Space>
      <Space>
        <Button onClick={() => void api.dataDictionary({ connectionId: ref.id, database: ref.database, schema: ref.schema }).then(setDoc).catch((err) => message.error(errorText(err)))}>数据字典</Button>
        <Button onClick={() => void api.analyzeSchema({ connectionId: ref.id, database: ref.database, schema: ref.schema }).then(setAnalysis).catch((err) => message.error(errorText(err)))}>结构分析</Button>
      </Space>
      {doc && <Input.TextArea rows={12} value={doc} readOnly />}
      <Grid result={analysis} />
    </div>
  );
}

export function Users() {
  const { message } = App.useApp();
  const profiles = useProfiles();
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [grid, setGrid] = useState<GridResult | null>(null);
  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Select style={{ minWidth: 200 }} placeholder="连接" value={id} options={profiles.map((item) => ({ value: item.id, label: item.name }))} onChange={(value) => { setId(value); setDatabase(defaultDb(profiles, value)); }} />
      <Space>
        <Button onClick={() => id && void api.listAccounts(id, database).then(setGrid).catch((err) => message.error(errorText(err)))}>账号</Button>
        <Button onClick={() => id && void api.serverActivity(id, database).then(setGrid).catch((err) => message.error(errorText(err)))}>会话 / 命令</Button>
      </Space>
      <Grid result={grid} />
    </div>
  );
}

export function CatalogSearch() {
  const { message } = App.useApp();
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [schema, setSchema] = useState("");
  const [query, setQuery] = useState("");
  const [grid, setGrid] = useState<GridResult | null>(null);
  const openData = useTabStore((state) => state.openData);
  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <ConnFields id={id} database={database} schema={schema} onChange={(patch) => {
        if (patch.id !== undefined) setId(patch.id);
        if (patch.database !== undefined) setDatabase(patch.database);
        if (patch.schema !== undefined) setSchema(patch.schema);
      }} />
      <Space>
        <Input value={query} placeholder="表名或列名" onChange={(event) => setQuery(event.target.value)} onPressEnter={() => id && void api.searchCatalog({ connectionId: id, database, schema }, query).then(setGrid).catch((err) => message.error(errorText(err)))} />
        <Button type="primary" onClick={() => id && void api.searchCatalog({ connectionId: id, database, schema }, query).then(setGrid).catch((err) => message.error(errorText(err)))}>搜索</Button>
      </Space>
      <Grid result={grid} />
      <Button disabled={!id || !grid?.rows[0]} onClick={() => {
        const row = grid?.rows[0];
        if (!id || !row) return;
        openData({ connectionId: id, database, schema: schema || database, table: String(row[1] ?? ""), readOnly: false });
      }}>打开第一条结果</Button>
    </div>
  );
}
