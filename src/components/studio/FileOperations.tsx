import { Alert, Button, Select, Space } from "antd";
import { useState } from "react";
import { api, pickOpenPath, pickSavePath, type FileExportResult } from "../../api";
import { useLongOperation } from "../../operations";
import { TaskProgress } from "../TaskProgress";
import { ConnFields, objectLabel, useDestructiveConfirm, useOperation, useProfiles } from "./shared";

interface Feedback {
  type: "success" | "warning";
  text: string;
  note?: string;
}

function exportFeedback(result: FileExportResult): Feedback {
  const dataOnly = result.kind === "data-only";
  const title = dataOnly ? "已导出数据脚本（非完整备份）"
    : result.kind === "native-backup" ? "原生备份命令已完成"
    : result.kind === "sqlite-script" ? "已导出 SQLite 结构与数据脚本"
    : `已导出 ${result.rowCount} 行`;
  return {
    type: result.truncated || dataOnly ? "warning" : "success",
    text: `${title}${result.truncated ? "，仅包含部分数据" : ""}：${result.path}`,
    note: result.note || (dataOnly ? "仅包含数据 INSERT 语句，不包含完整结构，不保证一致性快照。" : undefined),
  };
}

export function ImportExport() {
  const [ref, setRef] = useState({ id: "", database: "", schema: "", table: "" });
  const [format, setFormat] = useState("json");
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const { busy, run } = useOperation();
  const task = useLongOperation();
  const ready = !!ref.id && !!ref.table;

  async function transfer(exporting: boolean) {
    await run(exporting ? "export" : "import", async () => {
      const path = exporting ? await pickSavePath(`${ref.table || "export"}.${format}`) : await pickOpenPath();
      if (!path) return;
      setFeedback(null);
      const request = { connectionId: ref.id, database: ref.database, schema: ref.schema, table: ref.table, format, path };
      if (exporting) setFeedback(exportFeedback(await task.execute("导出文件", (operationId) => api.exportTableFile(request, operationId))));
      else setFeedback({ type: "success", text: `已导入 ${await task.execute("导入文件", (operationId) => api.importTableFile(request, operationId))} 行` });
    });
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="info" showIcon message="支持 JSON、XML、TXT、CSV、XLSX、DBF、PDF、Parquet。Parquet 单元格按文本保存，可读写 Snappy、gzip、zstd 和 lz4。Access 需要 mdb-export，ODBC 需要本机 isql，DSN 写在导入路径里。" />
      <ConnFields {...ref} applySeed disabled={!!busy} onChange={(patch) => { setRef({ ...ref, ...patch }); setFeedback(null); }} />
      <Select disabled={!!busy} value={format} style={{ width: 160 }} options={["json", "xml", "txt", "csv", "xlsx", "dbf", "pdf", "parquet", "mdb", "odbc"].map((value) => ({ value, label: value.toUpperCase() }))} onChange={setFormat} />
      <Space>
        <Button disabled={!!busy || !ready} loading={busy === "export"} onClick={() => void transfer(true)}>导出</Button>
        <Button disabled={!!busy || !ready} loading={busy === "import"} onClick={() => void transfer(false)}>导入</Button>
      </Space>
      <TaskProgress task={task} />
      {feedback && <Alert type={feedback.type} showIcon message={feedback.text} description={feedback.note} />}
    </div>
  );
}

export function Backup() {
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [schema, setSchema] = useState("");
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const { busy, run } = useOperation();
  const task = useLongOperation();
  const profiles = useProfiles();
  const confirm = useDestructiveConfirm();

  async function backup() {
    await run("backup", async () => {
      if (!id) return;
      const path = await pickSavePath("backup.sql");
      if (!path) return;
      setFeedback(null);
      setFeedback(exportFeedback(await task.execute("数据库备份", (operationId) => api.backupSql({ connectionId: id, database, schema, path }, operationId))));
    });
  }

  async function restore() {
    await run("restore", async () => {
      if (!id) return;
      const path = await pickOpenPath();
      if (!path) return;
      const target = objectLabel({ id, database, schema, table: "" }, profiles);
      if (!await confirm("确认还原 / 执行 SQL", `目标：${target}\n文件：${path}\n将执行文件中的语句或还原命令，可能覆盖现有结构和数据。`)) return;
      setFeedback(null);
      setFeedback({ type: "success", text: await task.execute("恢复数据库", (operationId) => api.restoreSql({ connectionId: id, database, path }, operationId)) });
    });
  }

  async function convert() {
    await run("convert", async () => {
      const source = await pickOpenPath();
      if (!source) return;
      const destination = await pickSavePath("converted.sql");
      if (!destination) return;
      setFeedback(null);
      setFeedback({ type: "success", text: await task.execute("转换备份", (operationId) => api.convertBackup(source, destination, operationId)) });
    });
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="info" showIcon message="自动模式优先使用 mysqldump、pg_dump、mongodump；SQL Server 提交 BACKUP DATABASE，路径必须在服务器上。SQLite 可导出结构与数据脚本。其他引擎缺少原生工具时只导出数据 INSERT 脚本，不含完整结构，也不保证一致性快照。结果会标明实际产物。" />
      <ConnFields id={id} database={database} schema={schema} applySeed disabled={!!busy} onChange={(patch) => {
        if (patch.id !== undefined) setId(patch.id);
        if (patch.database !== undefined) setDatabase(patch.database);
        if (patch.schema !== undefined) setSchema(patch.schema);
        setFeedback(null);
      }} />
      <Space>
        <Button type="primary" disabled={!!busy || !id} loading={busy === "backup"} onClick={() => void backup()}>备份 / 导出脚本</Button>
        <Button disabled={!!busy || !id} loading={busy === "restore"} onClick={() => void restore()}>还原 / 执行 SQL</Button>
        <Button disabled={!!busy} loading={busy === "convert"} onClick={() => void convert()}>备份转 SQL</Button>
      </Space>
      <TaskProgress task={task} />
      {feedback && <Alert type={feedback.type} showIcon message={feedback.text} description={feedback.note} />}
    </div>
  );
}
