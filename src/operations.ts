import { createContext, useContext, useEffect, useRef, useState } from "react";
import { create } from "zustand";
import { api, errorText, type OperationSnapshot } from "./api";

export const OperationOwnerContext = createContext<string | undefined>(undefined);

interface ActiveOperation {
  key: string;
  ownerId?: string;
  cancel: () => Promise<void>;
  done: Promise<void>;
}

export const useOperationStore = create<{ active: ActiveOperation[] }>(() => ({ active: [] }));

export async function cancelTabOperations(ownerId: string): Promise<void> {
  const tasks = useOperationStore.getState().active.filter((task) => task.ownerId === ownerId);
  await Promise.all(tasks.map((task) => task.cancel()));
  await Promise.all(tasks.map((task) => task.done));
}

function terminal(snapshot: OperationSnapshot) {
  return ["succeeded", "failed", "cancelled"].includes(snapshot.status);
}

interface RunningOperation {
  key: string;
  id?: string;
  finished: boolean;
  settling: boolean;
  cancelWanted: boolean;
  cancelAcknowledged: boolean;
  cancelFlight?: Promise<void>;
  timer?: ReturnType<typeof setTimeout>;
  latest: OperationSnapshot | null;
}

/** One tracked invocation owns its polling, cancellation and tab lifetime. */
export function useLongOperation(ownerOverride?: string) {
  const ownerContext = useContext(OperationOwnerContext);
  const ownerId = ownerOverride ?? ownerContext;
  const mounted = useRef(true);
  const running = useRef<RunningOperation | null>(null);
  const [active, setActive] = useState(false);
  const [snapshot, setSnapshot] = useState<OperationSnapshot | null>(null);
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [cancelPending, setCancelPending] = useState(false);

  function update(run: RunningOperation, next: OperationSnapshot) {
    if (running.current !== run || run.finished || next.id !== run.id) return;
    if (run.latest && terminal(run.latest) && !terminal(next)) return;
    const value = { ...next, cancelRequested: next.cancelRequested || run.cancelAcknowledged };
    run.latest = value;
    if (mounted.current) {
      setSnapshot(value);
      if (terminal(value)) { setCancelError(null); setCancelPending(false); }
    }
  }

  async function requestCancel(run: RunningOperation): Promise<void> {
    if (run.finished || (run.latest && terminal(run.latest))) return;
    run.cancelWanted = true;
    if (mounted.current) setCancelPending(true);
    if (!run.id) return;
    if (run.cancelFlight) return run.cancelFlight;
    if (run.cancelAcknowledged) return;
    if (mounted.current) setCancelError(null);
    run.cancelFlight = (async () => {
      try {
        const next = await api.cancelOperation(run.id!);
        run.cancelAcknowledged = next.cancelRequested || next.status === "cancelled";
        update(run, next);
      } catch (error) {
        run.cancelWanted = false;
        if (mounted.current) setCancelPending(false);
        if (mounted.current && running.current === run) setCancelError(`取消请求失败：${errorText(error)}`);
        throw error;
      } finally {
        run.cancelFlight = undefined;
      }
    })();
    return run.cancelFlight;
  }

  async function cancel() {
    const run = running.current;
    if (run) await requestCancel(run);
  }

  async function execute<T>(label: string, action: (id: string) => Promise<T>): Promise<T> {
    if (running.current) throw new Error("当前任务尚未结束，请等待后再操作");
    const run: RunningOperation = { key: crypto.randomUUID(), finished: false, settling: false, cancelWanted: false, cancelAcknowledged: false, latest: null };
    running.current = run;
    if (mounted.current) { setActive(true); setSnapshot(null); setCancelError(null); setCancelPending(false); }
    let resolveDone!: () => void;
    const done = new Promise<void>((resolve) => { resolveDone = resolve; });
    useOperationStore.setState((state) => ({ active: [...state.active, { key: run.key, ownerId, cancel: () => requestCancel(run), done }] }));
    let failed = false;
    const poll = async () => {
      if (run.finished || run.settling || !run.id) return;
      try { update(run, await api.operationStatus(run.id)); }
      catch { /* A transient polling failure never means the worker stopped. */ }
      if (!run.finished && !run.settling) run.timer = setTimeout(() => void poll(), 400);
    };
    try {
      const created = await api.createOperation(label);
      run.id = created.id;
      update(run, created);
      if (run.cancelWanted) await requestCancel(run).catch(() => {});
      run.timer = setTimeout(() => void poll(), 400);
      return await action(created.id);
    } catch (error) {
      failed = true;
      throw error;
    } finally {
      run.settling = true;
      if (run.timer) clearTimeout(run.timer);
      if (run.id) {
        // A returned/rejected IPC call alone cannot prove a worker has stopped.
        // Keep the write lock until the backend confirms its terminal state.
        while (!run.latest || !terminal(run.latest)) {
          try {
            update(run, await api.operationStatus(run.id));
            if (run.latest && terminal(run.latest)) break;
            if (failed && run.latest?.status === "queued") await requestCancel(run).catch(() => {});
          } catch (error) {
            if (error && typeof error === "object" && "kind" in error && error.kind === "operationMissing") {
              update(run, {
                id: run.id, label, status: "failed", phase: "结果需要核对", completed: run.latest?.completed ?? 0,
                total: run.latest?.total ?? null, committed: run.latest?.committed ?? 0, batches: run.latest?.batches ?? 0,
                cancelRequested: run.cancelAcknowledged, interruptible: false,
                message: "后台任务记录已不可用，请核对数据库和输出文件；不能仅凭此状态确认成功或回滚。",
              });
              break;
            }
            if (mounted.current) setCancelError(`正在确认后台是否结束，请勿重复操作：${errorText(error)}`);
          }
          if (!run.latest || !terminal(run.latest)) await new Promise((resolve) => setTimeout(resolve, 400));
        }
      }
      run.finished = true;
      if (run.timer) clearTimeout(run.timer);
      if (run.id && run.latest && terminal(run.latest)) {
        try { await api.releaseOperation(run.id); } catch { /* Backend expiry can release completed records later. */ }
      }
      if (running.current === run) running.current = null;
      useOperationStore.setState((state) => ({ active: state.active.filter((task) => task.key !== run.key) }));
      resolveDone();
      if (mounted.current) { setActive(false); setCancelPending(false); }
    }
  }

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      const run = running.current;
      if (run && !run.finished) void requestCancel(run).catch(() => {});
    };
  }, []);

  return { active, snapshot, cancelError, cancelPending, execute, cancel };
}

export type LongOperation = ReturnType<typeof useLongOperation>;
