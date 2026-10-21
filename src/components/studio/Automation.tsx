import { App, Alert, Button, Input, InputNumber, Select, Space, Table, Tag } from "antd";
import { useEffect, useState } from "react";
import { api, errorText, pickSavePath, type Job } from "../../api";
import { defaultDb, useOperation, useProfiles } from "./shared";

function newJob(): Job {
  return {
    id: "", name: "新任务", kind: "query", connectionId: "", database: "", schema: "", table: "books",
    sql: "SELECT COUNT(*) FROM books", path: "", intervalSecs: 3600, nextRunAt: 0,
    enabled: false, lastMessage: "", notifyEmail: "", profileIds: [],
  };
}

export function Jobs() {
  const { message, modal } = App.useApp();
  const profiles = useProfiles();
  const [jobs, setJobs] = useState<Job[]>([]);
  const [draft, setDraft] = useState<Job>(newJob);
  const [smtpHost, setSmtpHost] = useState("");
  const [smtpPort, setSmtpPort] = useState(587);
  const [smtpUser, setSmtpUser] = useState("");
  const [smtpPass, setSmtpPass] = useState("");
  const [smtpFrom, setSmtpFrom] = useState("");
  const { busy, run } = useOperation();

  async function reload() {
    setJobs(await api.listJobs());
  }
  useEffect(() => { void reload().catch((error) => message.error(errorText(error))); }, []);

  function patch(change: Partial<Job>) {
    setDraft((current) => ({ ...current, ...change }));
  }

  function edit(job: Job) {
    setDraft({ ...job, profileIds: [...(job.profileIds || [])] });
  }

  async function save(enabled: boolean) {
    await run(enabled ? "enable" : "save", async () => {
      const profileIds = draft.profileIds || [];
      if (!draft.name.trim()) throw new Error("请填写任务名称");
      if (!["query", "backup", "export"].includes(draft.kind)) throw new Error("请重新选择任务类型");
      if (enabled) {
        const ids = [draft.connectionId, ...profileIds];
        if (ids.some((id) => !profiles.some((profile) => profile.id === id))) throw new Error("请选择有效连接，并检查批量连接 ID");
        if (draft.kind === "query" && !draft.sql.trim()) throw new Error("查询任务需要 SQL");
        if (draft.kind === "export" && (!draft.table.trim() || !draft.path.trim())) throw new Error("导出任务需要表名和输出路径");
        const confirmed = await new Promise<boolean>((resolve) => {
          modal.confirm({
            title: "确认保存并启用任务",
            content: <div className="space-y-2 break-all">
              <div>任务：{draft.name}</div>
              <div>连接：{profiles.find((item) => item.id === draft.connectionId)?.name} / {draft.database || "连接默认数据库"}</div>
              <div>模式 / 表：{draft.schema || "默认模式"} / {draft.table || "未指定"}</div>
              <div>输出路径：{draft.path || "自动路径（备份）"}</div>
              {!!profileIds.length && <div>其他连接：{profileIds.map((id) => profiles.find((item) => item.id === id)?.name || id).join("、")}</div>}
              {draft.kind === "query" && <pre className="max-h-48 overflow-auto whitespace-pre-wrap">{draft.sql}</pre>}
              <div>启用后每 {draft.intervalSecs} 秒运行，SQL 可能修改数据库。请确认已检查以上内容。</div>
            </div>,
            okText: "保存并启用", cancelText: "取消",
            onOk: () => { resolve(true); }, onCancel: () => { resolve(false); },
          });
        });
        if (!confirmed) return;
      }
      const saved = await api.saveJob({ ...draft, profileIds, enabled, nextRunAt: 0 });
      setDraft(saved);
      await reload();
      message.success(enabled ? "任务已保存并启用，将按间隔运行" : "任务已保存为暂停状态");
    });
  }

  async function pause(job: Job) {
    await run(`pause:${job.id}`, async () => {
      const saved = await api.saveJob({ ...job, enabled: false });
      if (draft.id === saved.id) setDraft((current) => ({ ...current, enabled: false }));
      await reload();
      message.success("任务已暂停；正在执行的操作可能仍会完成");
    });
  }

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="info" showIcon message="窗口关闭后会留在托盘里继续每 15 秒检查。系统计划任务通过 --run-jobs 执行到期任务。新建或导入任务先保持暂停，查看连接、SQL 和输出路径后再明确启用。邮件走你自己的 SMTP。" />
      <Space wrap>
        <Button disabled={!!busy} onClick={() => edit(newJob())}>新建任务</Button>
        <Button disabled={!!busy} loading={busy === "reload"} onClick={() => void run("reload", reload)}>刷新任务</Button>
        <Tag>{draft.id ? `编辑任务：${draft.id}` : "新任务"}</Tag>
        <Tag color={draft.enabled ? "green" : "default"}>{draft.enabled ? "已启用" : "已暂停"}</Tag>
      </Space>
      <Space wrap>
        <Input disabled={!!busy} value={draft.name} onChange={(event) => patch({ name: event.target.value })} placeholder="任务名称" aria-label="任务名称" />
        <Select disabled={!!busy} value={draft.kind} style={{ width: 120 }} options={[{ value: "query", label: "查询 / 执行 SQL" }, { value: "backup", label: "备份" }, { value: "export", label: "导出 CSV" }]} onChange={(kind) => patch({ kind })} />
        <Select disabled={!!busy} style={{ minWidth: 180 }} placeholder="连接" value={draft.connectionId || undefined} options={profiles.map((item) => ({ value: item.id, label: item.name }))} onChange={(connectionId) => patch({ connectionId, database: defaultDb(profiles, connectionId) })} />
        <InputNumber disabled={!!busy} min={15} precision={0} value={draft.intervalSecs} onChange={(value) => patch({ intervalSecs: value ?? 3600 })} addonAfter="秒" />
      </Space>
      <Space wrap>
        <Input disabled={!!busy} value={draft.database} onChange={(event) => patch({ database: event.target.value })} placeholder="数据库（空为连接默认值）" aria-label="数据库" />
        <Input disabled={!!busy} value={draft.schema} onChange={(event) => patch({ schema: event.target.value })} placeholder="模式" aria-label="模式" />
        <Input disabled={!!busy} value={draft.table} onChange={(event) => patch({ table: event.target.value })} placeholder="表（导出任务必填）" aria-label="表" />
      </Space>
      <Input.TextArea disabled={!!busy} rows={4} value={draft.sql} onChange={(event) => patch({ sql: event.target.value })} placeholder="任务 SQL（查询 / 执行 SQL 类型会执行此内容）" aria-label="任务 SQL" />
      <Space.Compact className="w-full max-w-xl">
        <Input disabled={!!busy} value={draft.path} onChange={(event) => patch({ path: event.target.value })} placeholder="输出路径（导出必填；备份留空使用配置目录）" aria-label="输出路径" />
        <Button disabled={!!busy} onClick={() => void pickSavePath(draft.path || "export.csv").then((path) => { if (path) patch({ path }); })}>浏览</Button>
      </Space.Compact>
      <Input disabled={!!busy} value={draft.notifyEmail || ""} onChange={(event) => patch({ notifyEmail: event.target.value })} placeholder="通知邮箱，可空" />
      <Select
        disabled={!!busy}
        mode="multiple"
        style={{ width: "100%" }}
        placeholder="同时在这些连接上执行"
        value={draft.profileIds || []}
        options={profiles.filter((item) => item.id !== draft.connectionId).map((item) => ({ value: item.id, label: item.name }))}
        onChange={(profileIds) => patch({ profileIds })}
      />
      <Space wrap>
        <Button disabled={!!busy} loading={busy === "save"} onClick={() => void save(false)}>保存为暂停</Button>
        <Button type="primary" disabled={!!busy} loading={busy === "enable"} onClick={() => void save(true)}>保存并启用</Button>
        <Button disabled={!!busy} onClick={() => void run("schedule", async () => { message.success(await api.installSchedule(1)); })}>安装系统计划</Button>
      </Space>
      <div className="text-xs text-[var(--xb-muted)]">系统计划每分钟检查到期任务，执行间隔以各任务已保存的设置为准。</div>
      <Table size="small" pagination={false} rowKey="id" dataSource={jobs} columns={[
        { title: "名称", dataIndex: "name" },
        { title: "类型", dataIndex: "kind" },
        { title: "状态", render: (_: unknown, job: Job) => <Tag color={job.enabled ? "green" : "default"}>{job.enabled ? "已启用" : "已暂停"}</Tag> },
        { title: "连接", render: (_: unknown, job: Job) => profiles.find((item) => item.id === job.connectionId)?.name || job.connectionId },
        { title: "上次", dataIndex: "lastMessage" },
        { title: "操作", render: (_: unknown, job: Job) => <Space wrap>
          <Button size="small" disabled={!!busy} onClick={() => edit(job)}>查看 / 编辑</Button>
          {job.enabled && <Button size="small" disabled={!!busy} loading={busy === `pause:${job.id}`} onClick={() => void pause(job)}>暂停</Button>}
          <Button size="small" danger disabled={!!busy} onClick={() => modal.confirm({
            title: `删除任务「${job.name}」？`,
            content: "只删除本机任务配置，不会撤销已经执行过的 SQL。",
            okText: "删除",
            okButtonProps: { danger: true },
            cancelText: "取消",
            onOk: () => run(`delete:${job.id}`, async () => {
              await api.deleteJob(job.id);
              if (draft.id === job.id) edit(newJob());
              await reload();
            }),
          })}>删除</Button>
        </Space> },
      ]} />
      <Space wrap>
        <Input value={smtpHost} onChange={(event) => setSmtpHost(event.target.value)} placeholder="SMTP 主机" />
        <InputNumber value={smtpPort} onChange={(value) => setSmtpPort(value ?? 587)} />
        <Input value={smtpUser} onChange={(event) => setSmtpUser(event.target.value)} placeholder="SMTP 用户" />
        <Input.Password value={smtpPass} onChange={(event) => setSmtpPass(event.target.value)} placeholder="SMTP 密码" />
        <Input value={smtpFrom} onChange={(event) => setSmtpFrom(event.target.value)} placeholder="发件人" />
        <Button disabled={!!busy} loading={busy === "smtp"} onClick={() => void run("smtp", async () => {
          await api.saveMailSettings({ host: smtpHost, port: smtpPort, username: smtpUser, password: smtpPass, from: smtpFrom, starttls: smtpPort !== 465 });
          setSmtpPass(""); message.success("SMTP 已保存");
        })}>保存 SMTP</Button>
      </Space>
    </div>
  );
}
