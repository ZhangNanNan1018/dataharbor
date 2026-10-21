import { App, Alert, Button, Space, Switch } from "antd";
import { useState } from "react";
import { api, type SyncResult } from "../../api";
import { useLongOperation } from "../../operations";
import { TaskProgress } from "../TaskProgress";
import { ConnFields, objectLabel, pairRequest, useDestructiveConfirm, useOperation, useProfiles, type ObjectSelection } from "./shared";

function summarize(statements: { kind: string; warning: boolean }[]) {
  const count = (prefix: string) => statements.filter((item) => item.kind === prefix || item.kind.startsWith(`${prefix}-`)).length;
  const parts = [
    ["新增列", count("add-column")],
    ["删除列", count("drop-column")],
    ["修改类型", count("modify")],
    ["索引", count("index") + count("add-index") + count("drop-index")],
    ["外键", count("fk") + count("add-fk") + count("drop-fk")],
    ["视图", count("view")],
    ["警告", statements.filter((item) => item.warning).length],
  ].filter((item) => Number(item[1]) > 0).map(([label, value]) => `${label} ${value}`);
  return parts.length ? parts.join(" · ") : "没有结构差异";
}

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
      <ConnFields {...source} legend="源" applySeed disabled={!!busy} onChange={(patch) => changeObject("source", patch)} />
      <div>目标</div>
      <ConnFields {...target} legend="目标" disabled={!!busy} onChange={(patch) => changeObject("target", patch)} />
      <Space>
        <span>允许危险结构变更（删除 / 改类型 / 替换视图），启用后仍需勾选并确认。切换后需要重新对比。</span>
        <Switch checked={destructive} disabled={!!busy} onChange={(value) => { setDestructive(value); setStatements([]); setNotes([]); }} />
        <Button disabled={!!busy || !ready} loading={busy === "compare"} onClick={() => void run(false)}>对比</Button>
        <Button type="primary" disabled={!!busy || !ready || !statements.some((item) => item.picked)} loading={busy === "apply"} onClick={() => void run(true)}>执行勾选语句</Button>
      </Space>
      {statements.length > 0 && (
        <div className="text-xs text-[var(--xb-muted)]">
          {summarize(statements)}
        </div>
      )}
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
  const task = useLongOperation();
  const confirm = useDestructiveConfirm();
  const ready = !!source.id && !!source.table && !!target.id && !!target.table;

  async function transfer(sync: boolean, preview = false) {
    await run(preview ? (sync ? "preview-sync" : "preview-transfer") : sync ? "sync" : "transfer", async () => {
      if (!preview && sync && deleteMissing) {
        const ok = await confirm("确认同步并删除目标多出的行", `源：${objectLabel(source, profiles)}\n目标：${objectLabel(target, profiles)}\n按主键插入和更新目标，并删除目标中源表没有的行。此操作会修改目标数据。`);
        if (!ok) return;
      }
      setResult(null);
      const request = { ...pairRequest(source, target), deleteMissing: sync && deleteMissing, preview };
      const label = preview ? (sync ? "预览同步" : "预览传输") : sync ? "全量数据同步" : "数据传输";
      setResult(await task.execute(label, (operationId) => sync ? api.syncRows(request, operationId) : api.transferRows(request, operationId)));
    });
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <div>源</div>
      <ConnFields {...source} legend="源" applySeed disabled={!!busy} onChange={(patch) => { setSource({ ...source, ...patch }); setResult(null); }} />
      <div>目标</div>
      <ConnFields {...target} legend="目标" disabled={!!busy} onChange={(patch) => { setTarget({ ...target, ...patch }); setResult(null); }} />
      <Space>
        <span>同步时删除目标多出的行</span>
        <Switch checked={deleteMissing} disabled={!!busy} onChange={(value) => { setDeleteMissing(value); setResult(null); }} />
      </Space>
      <Space wrap>
        <Button disabled={!!busy || !ready} loading={busy === "preview-transfer"} onClick={() => void transfer(false, true)}>预览插入</Button>
        <Button disabled={!!busy || !ready} loading={busy === "preview-sync"} onClick={() => void transfer(true, true)}>预览同步</Button>
        <Button disabled={!!busy || !ready} loading={busy === "transfer"} onClick={() => void transfer(false)}>传输插入</Button>
        <Button type="primary" disabled={!!busy || !ready} loading={busy === "sync"} onClick={() => void transfer(true)}>按主键同步</Button>
      </Space>
      <TaskProgress task={task} />
      {result && <Alert type={result.note.startsWith("预览未写入") ? "info" : result.truncated ? "warning" : "success"} showIcon message={`${result.note.startsWith("预览未写入") ? "尚未写入。" : result.truncated ? "仅处理了部分数据。" : ""}插入 ${result.inserted}，更新 ${result.updated}，删除 ${result.deleted}。`} description={result.note} />}
    </div>
  );
}
