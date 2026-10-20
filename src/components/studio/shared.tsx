import { App, Input, Select, Space, Table } from "antd";
import { useRef, useState } from "react";
import { errorText, type GridResult, type PairRequest } from "../../api";
import { engineLabel } from "../../engines";
import { useConnectionStore } from "../../stores/useConnectionStore";
import { type ConnectionPublic, type JsonValue } from "../../types";

export function useProfiles() {
  return useConnectionStore((state) => state.profiles);
}

export function ConnFields({
  id,
  database,
  schema,
  table,
  disabled = false,
  onChange,
}: {
  id?: string;
  database?: string;
  schema?: string;
  table?: string;
  disabled?: boolean;
  onChange: (patch: { id?: string; database?: string; schema?: string; table?: string }) => void;
}) {
  const profiles = useProfiles();
  const profile = profiles.find((item) => item.id === id);
  return (
    <Space wrap>
      <Select
        disabled={disabled}
        style={{ minWidth: 180 }}
        placeholder="连接"
        value={id}
        options={profiles.map((item) => ({ value: item.id, label: `${item.name} · ${engineLabel(item.engine)}` }))}
        onChange={(value) => onChange({ id: value, database: value ? defaultDb(profiles, value) : "", schema: "", table: "" })}
      />
      <Input disabled={disabled} style={{ width: 140 }} placeholder="数据库" value={database} onChange={(event) => onChange({ database: event.target.value })} />
      <Input
        disabled={disabled}
        style={{ width: 120 }}
        placeholder={profile?.engine === "postgres" ? "public" : "模式"}
        value={schema}
        onChange={(event) => onChange({ schema: event.target.value })}
      />
      <Input disabled={disabled} style={{ width: 140 }} placeholder="表" value={table} onChange={(event) => onChange({ table: event.target.value })} />
    </Space>
  );
}

export interface ObjectSelection {
  id: string;
  database: string;
  schema: string;
  table: string;
}

export function pairRequest(source: ObjectSelection, target: ObjectSelection): PairRequest {
  return {
    sourceId: source.id, sourceDatabase: source.database, sourceSchema: source.schema, sourceTable: source.table,
    targetId: target.id, targetDatabase: target.database, targetSchema: target.schema, targetTable: target.table,
  };
}

export function objectLabel(selection: ObjectSelection, profiles: ConnectionPublic[]): string {
  const profile = profiles.find((item) => item.id === selection.id);
  const address = profile?.engine === "sqlite" ? profile.filePath : profile ? `${profile.host}:${profile.port}` : selection.id;
  return `${profile?.name || selection.id} (${address}) / ${[selection.database, selection.schema, selection.table].filter(Boolean).join(" / ") || "未指定对象"}`;
}

/** A ref locks immediately, including file pickers and confirmations before React rerenders. */
export function useOperation() {
  const { message } = App.useApp();
  const active = useRef(false);
  const [busy, setBusy] = useState<string | null>(null);
  async function run(name: string, action: () => Promise<void>) {
    if (active.current) return;
    active.current = true;
    setBusy(name);
    try {
      await action();
    } catch (error) {
      message.error(errorText(error));
    } finally {
      active.current = false;
      setBusy(null);
    }
  }
  return { busy, run };
}

export function useDestructiveConfirm() {
  const { modal } = App.useApp();
  return (title: string, details: string) => new Promise<boolean>((resolve) => {
    modal.confirm({
      title,
      content: <div className="whitespace-pre-wrap break-all">{details}</div>,
      okText: "确认执行",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: () => { resolve(true); },
      onCancel: () => { resolve(false); },
    });
  });
}

export function defaultDb(profiles: ConnectionPublic[], id: string): string {
  const profile = profiles.find((item) => item.id === id);
  if (!profile) return "";
  if (profile.engine === "sqlite") return "main";
  return profile.database || "";
}

export function Grid({ result }: { result: GridResult | null }) {
  if (!result) return null;
  return (
    <div className="mt-3">
      {result.note && <p className="mb-2 text-xs text-[var(--xb-muted)]">{result.note}</p>}
      <Table
        size="small"
        pagination={{ pageSize: 20 }}
        scroll={{ x: "max-content" }}
        dataSource={result.rows.map((row, index) => ({ key: index, cells: row }))}
        columns={result.columns.map((name, index) => ({
          title: name,
          key: name + index,
          render: (_: unknown, record: { cells: JsonValue[] }) => String(record.cells[index] ?? ""),
        }))}
      />
    </div>
  );
}
