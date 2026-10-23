import { Alert, App, Button, Space, Table, Tag } from "antd";
import { useEffect, useState } from "react";
import { api, errorText, type Job, type RunRecord } from "../../api";

const STATUS: Record<string, string> = { running: "运行中", succeeded: "已完成", failed: "失败", cancelled: "已取消", unknown: "结果待核对" };

export function ExecutionHistory({ jobs }: { jobs: Job[] }) {
  const { message } = App.useApp();
  const [records, setRecords] = useState<RunRecord[]>([]);
  const [loading, setLoading] = useState(false);
  async function reload() {
    setLoading(true);
    try { setRecords(await api.listRunHistory()); }
    catch (error) { message.error(errorText(error)); }
    finally { setLoading(false); }
  }
  useEffect(() => { void reload(); }, []);
  return <div className="space-y-2">
    <Space><strong>执行记录</strong><Button size="small" loading={loading} onClick={() => void reload()}>刷新记录</Button></Space>
    {records.some((record) => record.status === "unknown") && <Alert type="warning" showIcon
      message="有异常退出的任务需要核对结果"
      description="数据库可能已经提交部分操作。请先核对目标数据或输出文件，再决定是否重新执行；未确认结果的定时任务会暂停。" />}
    <Table<RunRecord> size="small" rowKey="id" loading={loading} dataSource={records} pagination={{ pageSize: 10, showSizeChanger: false }} columns={[
      { title: "开始时间", render: (_, row) => new Date(row.startedAt * 1000).toLocaleString() },
      { title: "任务", render: (_, row) => row.jobId ? jobs.find((job) => job.id === row.jobId)?.name || "已删除的定时任务" : "手动操作" },
      { title: "状态", render: (_, row) => <Tag color={row.status === "unknown" ? "orange" : row.status === "failed" ? "red" : row.status === "succeeded" ? "green" : "default"}>{STATUS[row.status] || row.status}</Tag> },
      { title: "已知提交", render: (_, row) => `${row.committed} 条 / ${row.batches} 批` },
      { title: "耗时", render: (_, row) => row.finishedAt == null ? "—" : `${Math.max(0, row.finishedAt - row.startedAt)} 秒` },
      { title: "记录编号", render: (_, row) => <span title={row.id}>{row.id.slice(0, 8)}</span> },
    ]} />
    <div className="text-xs text-[var(--xb-muted)]">显示最近 200 条记录。记录不包含 SQL、密码和查询结果；提交数量为已确认的下限。</div>
  </div>;
}
