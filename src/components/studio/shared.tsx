import { App, Input, Select, Space, Table } from "antd";
import { createContext, useContext, useEffect, useRef, useState } from "react";
import { api, errorText, isOperationCancelled, type GridResult, type PairRequest } from "../../api";
import { engineLabel, usesSchema } from "../../engines";
import { useConnectionStore } from "../../stores/useConnectionStore";
import { type ConnectionPublic, type JsonValue, type StudioContext } from "../../types";
import { ensureConnection } from "../../connectionReady";

export const StudioSeedContext = createContext<StudioContext | null>(null);

export function useProfiles() {
  return useConnectionStore((state) => state.profiles);
}

export function ConnFields({
  id,
  database,
  schema,
  table,
  legend = "",
  applySeed = false,
  disabled = false,
  onChange,
}: {
  id?: string;
  database?: string;
  schema?: string;
  table?: string;
  legend?: string;
  /** Fill this field from the object that opened the tool. */
  applySeed?: boolean;
  disabled?: boolean;
  onChange: (patch: { id?: string; database?: string; schema?: string; table?: string }) => void;
}) {
  const { message } = App.useApp();
  const profiles = useProfiles();
  const profile = profiles.find((item) => item.id === id);
  const schemaEngine = usesSchema(profile?.engine);
  const showTable = table !== undefined;
  const seed = useContext(StudioSeedContext);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const showTableRef = useRef(showTable);
  showTableRef.current = showTable;
  const seedRef = useRef(seed);
  seedRef.current = seed;
  const [databases, setDatabases] = useState<string[]>([]);
  const [schemas, setSchemas] = useState<string[]>([]);
  const [tables, setTables] = useState<string[]>([]);
  const [tableLoaded, setTableLoaded] = useState(false);
  const [readyProfile, setReadyProfile] = useState<ConnectionPublic | null>(null);
  const ready = profile?.connected === true && readyProfile === profile;
  const seedKey = applySeed && seed ? `${seed.connectionId}|${seed.database}|${seed.schema}|${seed.table}` : "";

  useEffect(() => {
    const current = seedRef.current;
    if (!seedKey || !current?.connectionId) return;
    onChangeRef.current({
      id: current.connectionId,
      database: current.database,
      schema: current.schema,
      ...(showTableRef.current ? { table: current.table } : {}),
    });
  }, [seedKey]);

  useEffect(() => {
    setReadyProfile(null);
    if (!id || !profile) return;
    let cancelled = false;
    setDatabases([]);
    setSchemas([]);
    setTables([]);
    setTableLoaded(false);
    void (async () => {
      try {
        await ensureConnection(id);
        if (cancelled) return;
        const connected = useConnectionStore.getState().profiles.find((item) => item.id === id)!;
        setReadyProfile(connected);
        if (profile.engine === "sqlite") {
          if (database !== "main" || schema !== "main") onChangeRef.current({ database: "main", schema: "main" });
          return;
        }
        const names = await api.listDatabases(id);
        if (!cancelled) setDatabases(names);
      } catch (err) {
        if (!cancelled) message.error(errorText(err));
      }
    })();
    return () => {
      cancelled = true;
    };
    // Catalog reload follows the connection. Database and schema updates are handled below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, profile]);

  useEffect(() => {
    if (!id || !ready || !profile || profile.engine === "sqlite" || !database) return;
    if (!schemaEngine) {
      if (schema !== database) onChangeRef.current({ schema: database });
      return;
    }
    let cancelled = false;
    void api.listSchemas(id, database).then((names) => {
      if (cancelled) return;
      setSchemas(names);
      if (!schema && names.length === 1) onChangeRef.current({ schema: names[0] });
    }).catch((err) => {
      if (!cancelled) message.error(errorText(err));
    });
    return () => {
      cancelled = true;
    };
    // `message` is only used to report a failed catalog read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, ready, database, schema, schemaEngine]);

  useEffect(() => {
    if (!showTable || !id || !ready || !database) return;
    const schemaName = schemaEngine ? schema : database;
    if (!schemaName) return;
    let cancelled = false;
    setTables([]);
    setTableLoaded(false);
    void Promise.all([
      api.listTables(id, database, schemaName),
      api.listViews(id, database, schemaName).catch(() => [] as string[]),
    ]).then(([tableNames, viewNames]) => {
      if (cancelled) return;
      setTables([...tableNames, ...viewNames.filter((name) => !tableNames.includes(name))]);
      setTableLoaded(true);
    }).catch((err) => {
      if (cancelled) return;
      setTables([]);
      setTableLoaded(true);
      message.error(errorText(err));
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showTable, id, ready, database, schema, schemaEngine]);

  const name = (suffix: string) => (legend ? `${legend}${suffix}` : suffix);
  const manualTable = showTable && tableLoaded && tables.length === 0;
  return (
    <Space wrap>
      <Select
        aria-label={name("连接")}
        disabled={disabled}
        showSearch
        optionFilterProp="label"
        style={{ minWidth: 180 }}
        placeholder={name("连接")}
        value={id || undefined}
        options={profiles.map((item) => ({ value: item.id, label: `${item.name} · ${engineLabel(item.engine)}` }))}
        onChange={(value) => onChange({ id: value, database: value ? defaultDb(profiles, value) : "", schema: "", table: showTable ? "" : undefined })}
      />
      {profile?.engine !== "sqlite" && (
        <Select
          aria-label={name("数据库")}
          disabled={disabled || !id}
          showSearch
          optionFilterProp="label"
          style={{ minWidth: 140 }}
          placeholder={name("数据库")}
          value={database || undefined}
          options={withCurrent(databases, database)}
          onChange={(value) => onChange({ database: value, schema: "", ...(showTable ? { table: "" } : {}) })}
        />
      )}
      {schemaEngine && (
        <Select
          aria-label={name("模式")}
          disabled={disabled || !database}
          showSearch
          optionFilterProp="label"
          style={{ minWidth: 120 }}
          placeholder={name("模式")}
          value={schema || undefined}
          options={withCurrent(schemas, schema)}
          onChange={(value) => onChange({ schema: value, ...(showTable ? { table: "" } : {}) })}
        />
      )}
      {showTable && (manualTable ? (
        <Input
          aria-label={name("表")}
          disabled={disabled || !id}
          style={{ width: 160 }}
          placeholder={name("表")}
          value={table}
          onChange={(event) => onChange({ table: event.target.value })}
        />
      ) : (
        <Select
          aria-label={name("表")}
          disabled={disabled || !id || (schemaEngine ? !schema : !database)}
          showSearch
          optionFilterProp="label"
          style={{ minWidth: 160 }}
          placeholder={name("表")}
          value={table || undefined}
          options={withCurrent(tables, table)}
          onChange={(value) => onChange({ table: value })}
        />
      ))}
    </Space>
  );
}

function withCurrent(options: string[], current?: string): { value: string; label: string }[] {
  const values = current && !options.includes(current) ? [current, ...options] : options;
  return values.filter(Boolean).map((value) => ({ value, label: value }));
}

export interface ObjectSelection {
  id: string;
  database: string;
  schema: string;
  table: string;
}

export function pairRequest(source: ObjectSelection, target: ObjectSelection): PairRequest {
  return {
    apply: false, deleteMissing: false, allowDestructive: false, selectedIds: [], preview: false,
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
      if (!isOperationCancelled(error)) message.error(errorText(error));
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

export function Grid({ result, onOpen }: { result: GridResult | null; onOpen?: (row: JsonValue[]) => void }) {
  if (!result) return null;
  return (
    <div className="mt-3">
      {result.note && <p className="mb-2 text-xs text-[var(--xb-muted)]">{result.note}</p>}
      <Table
        size="small"
        pagination={{ pageSize: 20 }}
        scroll={{ x: "max-content" }}
        dataSource={result.rows.map((row, index) => ({ key: index, cells: row }))}
        onRow={onOpen ? (record) => ({ onClick: () => onOpen(record.cells), style: { cursor: "pointer" } }) : undefined}
        columns={result.columns.map((name, index) => ({
          title: name,
          key: name + index,
          render: (_: unknown, record: { cells: JsonValue[] }) => String(record.cells[index] ?? ""),
        }))}
      />
    </div>
  );
}
