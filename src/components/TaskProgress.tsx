import { Alert, Button, Progress, Space, Spin } from "antd";
import type { LongOperation } from "../operations";

export function TaskProgress({ task }: { task: Pick<LongOperation, "active" | "snapshot" | "cancelError" | "cancelPending" | "cancel"> }) {
  const { active, snapshot, cancelError } = task;
  if (!active && !snapshot) return null;
  const stopped = snapshot?.status === "cancelled";
  const failed = snapshot?.status === "failed";
  const backendFinished = snapshot && ["succeeded", "failed", "cancelled"].includes(snapshot.status);
  const cancelling = active && (snapshot?.cancelRequested || task.cancelPending);
  const title = active && backendFinished ? "后台已结束，正在接收结果" : cancelling ? snapshot?.cancelRequested ? "已请求取消，等待操作停止" : "正在请求取消" : active ? snapshot?.phase || snapshot?.label || "正在准备任务" : stopped ? "任务已取消" : failed ? "任务失败" : "任务已完成";
  const knownTotal = snapshot?.total !== null && snapshot?.total !== undefined;
  const percent = !knownTotal ? undefined : snapshot!.total! > 0 ? Math.min(100, Math.floor(snapshot!.completed / snapshot!.total! * 100)) : snapshot?.status === "succeeded" ? 100 : 0;
  return (
    <div className="my-2" role="status" aria-live="polite" aria-label="任务进度">
      <Alert
        type={failed ? "error" : stopped || cancelling ? "warning" : active ? "info" : "success"}
        showIcon
        message={<Space wrap>{active && <Spin size="small" />}{snapshot?.label && snapshot.label !== title && <span>{snapshot.label}</span>}<span>{title}</span>{active && !backendFinished && <Button size="small" disabled={Boolean(cancelling)} onClick={() => { void task.cancel().catch(() => {}); }}>取消任务</Button>}</Space>}
        description={<div>
          {knownTotal && <Progress percent={percent} size="small" status={failed ? "exception" : active ? "active" : "normal"} />}
          {snapshot && <div>本阶段已处理 {snapshot.completed.toLocaleString()}{knownTotal ? ` / ${snapshot.total!.toLocaleString()}` : "，总量待确认"}；已知已提交 {snapshot.committed.toLocaleString()} 条，{snapshot.batches.toLocaleString()} 批。</div>}
          {snapshot?.message && <div className="whitespace-pre-wrap break-words">{snapshot.message}</div>}
          {active && !snapshot?.interruptible && <div>取消后等待当前语句或批次结束，再停止后续操作。</div>}
          {stopped && <div>已提交数据不会自动撤销；被中断语句或原生工具的结果请另行核对。</div>}
          {cancelError && <div>{cancelError}</div>}
        </div>}
      />
    </div>
  );
}
