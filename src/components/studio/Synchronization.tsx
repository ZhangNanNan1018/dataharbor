import { App, Alert, Button, Space, Switch } from "antd";
import { useState } from "react";
import { api, type SyncResult } from "../../api";
import { ConnFields, objectLabel, pairRequest, useDestructiveConfirm, useOperation, useProfiles, type ObjectSelection } from "./shared";

function changesExistingStructure(statement: { kind: string }) {
  return statement.kind.startsWith("drop-") || statement.kind === "modify" || statement.kind === "view";
}

export function StructureSync() {
  const { message } = App.useApp();
  const [source, setSource] = useState({ id: "", database: "", schema: "", table: "" });
  const [target, setTarget] = useState({ id: "", database: "", schema: "", table: "" });
  const [notes, setNotes] = useState<string[]>([]);
  const [statements, setStatements] = useState<{ id: string; kind: string; sql: string; warning: boolean; picked: boolean }[]>([]);
  const [destructive, setDestructive] = useState(false);
  const profiles = useProfiles();
  const { busy, run: operate } = useOperation();
  const confirm = useDestructiveConfirm();
  const ready = !!source.id && !!source.table && !!target.id && !!target.table;

  function changeObject(which: "source" | "target", patch: Partial<ObjectSelection>) {
    if (which === "source") setSource((current) => ({ ...current, ...patch }));
    else setTarget((current) => ({ ...current, ...patch }));
    setStatements([]);
    setNotes([]);
  }

  async function run(apply: boolean) {
    await operate(apply ? "apply" : "compare", async () => {
      const selected = statements.filter((item) => item.picked);
      if (apply && selected.some(changesExistingStructure)) {
        const ok = await confirm("确认修改目标结构", `源：${objectLabel(source, profiles)}\n目标：${objectLabel(target, profiles)}\n将执行 ${selected.length} 条勾选语句，包含删除、类型修改或替换视图，可能丢失数据。\n\n${selected.map((item) => item.sql).join("\n\n")}`);
        if (!ok) return;
      }
      const result = await api.diffStructure({
        ...pairRequest(source, target),
        apply,
        allowDestructive: destructive,
        selectedIds: apply ? selected.map((item) => item.id) : [],
      });
      setNotes(result.notes);
      if (!apply) {
        setStatements((result.statements ?? []).map((item) => ({ ...item, picked: !item.warning && !changesExistingStructure(item) })));
      }
      if (apply) {
        setStatements([]);
        message.success(result.applied ? "已执行勾选的语句，请重新对比查看当前结构" : "没有可执行的语句");
      }
    });
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="info" showIcon message="对比列、索引、外键和视图。删列和改类型默认不勾选。SQLite 的类型变更和外键会标成警告。" />
      <div>源</div>
      <ConnFields {...source} disabled={!!busy} onChange={(patch) => changeObject("source", patch)} />
      <div>目标</div>
      <ConnFields {...target} disabled={!!busy} onChange={(patch) => changeObject("target", patch)} />
      <Space>
        <span>允许生成删除语句</span>
        <Switch checked={destructive} disabled={!!busy} onChange={(value) => { setDestructive(value); setStatements([]); setNotes([]); }} />
        <Button disabled={!!busy || !ready} loading={busy === "compare"} onClick={() => void run(false)}>对比</Button>
        <Button type="primary" disabled={!!busy || !ready || !statements.some((item) => item.picked)} loading={busy === "apply"} onClick={() => void run(true)}>执行勾选语句</Button>
      </Space>
      {notes.map((note) => <div key={note} className="text-xs text-[var(--xb-muted)]">{note}</div>)}
      {statements.map((item) => (
        <label key={item.id} className="block rounded border border-[var(--xb-line)] p-2 text-xs">
          <input
            type="checkbox"
            checked={item.picked}
            disabled={!!busy}
            onChange={(event) => setStatements((current) => current.map((row) => row.id === item.id ? { ...row, picked: event.target.checked } : row))}
          />
          <span className="ml-2">{item.kind}{item.warning ? " · 警告" : ""}</span>
          <pre className="mt-1 whitespace-pre-wrap">{item.sql}</pre>
        </label>
      ))}
    </div>
  );
}

export function DataSync() {
  const [source, setSource] = useState({ id: "", database: "", schema: "", table: "" });
  const [target, setTarget] = useState({ id: "", database: "", schema: "", table: "" });
  const [deleteMissing, setDeleteMissing] = useState(false);
  const [result, setResult] = useState<SyncResult | null>(null);
  const profiles = useProfiles();
  const { busy, run } = useOperation();
  const confirm = useDestructiveConfirm();
  const ready = !!source.id && !!source.table && !!target.id && !!target.table;

  async function transfer(sync: boolean) {
    await run(sync ? "sync" : "transfer", async () => {
      if (sync && deleteMissing) {
        const ok = await confirm("确认同步并删除目标多出的行", `源：${objectLabel(source, profiles)}\n目标：${objectLabel(target, profiles)}\n按主键插入和更新目标，并删除目标中源表没有的行。此操作会修改目标数据。`);
        if (!ok) return;
      }
      setResult(null);
      const request = { ...pairRequest(source, target), deleteMissing: sync && deleteMissing };
      setResult(await (sync ? api.syncRows(request) : api.transferRows(request)));
    });
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <div>源</div>
      <ConnFields {...source} disabled={!!busy} onChange={(patch) => { setSource({ ...source, ...patch }); setResult(null); }} />
      <div>目标</div>
      <ConnFields {...target} disabled={!!busy} onChange={(patch) => { setTarget({ ...target, ...patch }); setResult(null); }} />
      <Space>
        <span>同步时删除目标多出的行</span>
        <Switch checked={deleteMissing} disabled={!!busy} onChange={(value) => { setDeleteMissing(value); setResult(null); }} />
      </Space>
      <Space>
        <Button disabled={!!busy || !ready} loading={busy === "transfer"} onClick={() => void transfer(false)}>传输插入</Button>
        <Button type="primary" disabled={!!busy || !ready} loading={busy === "sync"} onClick={() => void transfer(true)}>按主键同步</Button>
      </Space>
      {result && <Alert type={result.truncated ? "warning" : "success"} showIcon message={`${result.truncated ? "仅处理了部分数据：" : ""}插入 ${result.inserted}，更新 ${result.updated}，删除 ${result.deleted}。`} description={result.note} />}
    </div>
  );
}
