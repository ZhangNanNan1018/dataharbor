import { App, Alert, Button, Input, InputNumber, Select, Space, Table } from "antd";
import { useEffect, useState } from "react";
import { api, errorText } from "../../api";
import { useTabStore } from "../../stores/useTabStore";
import { useProfiles, defaultDb } from "./shared";

export function RedisPubSub() {
  const { message } = App.useApp();
  const profiles = useProfiles().filter((item) => item.engine === "redis");
  const [id, setId] = useState<string>();
  const [channel, setChannel] = useState("orders");
  const [text, setText] = useState("hello");
  const [log, setLog] = useState<string[]>([]);
  const [listening, setListening] = useState(false);
  const [ttlKey, setTtlKey] = useState("orders");
  const [ttl, setTtl] = useState(60);

  useEffect(() => {
    if (!listening || !id) return;
    let stopped = false;
    const listen = () => {
      void api.redisListen(id, channel, 1500).then((items) => {
        if (stopped) return;
        if (items.length) setLog((current) => [...current, ...items].slice(-200));
        window.setTimeout(listen, 200);
      }).catch((err) => {
        if (!stopped) {
          setListening(false);
          message.error(errorText(err));
        }
      });
    };
    listen();
    return () => {
      stopped = true;
    };
  }, [listening, id, channel, message]);
  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="info" showIcon message="发布走当前连接。订阅会再开一条直连，SSH 隧道下需要 Redis 端口对本机可达。查询编辑器里也可以直接输入 Redis 命令。" />
      <Select style={{ minWidth: 200 }} placeholder="Redis 连接" value={id} options={profiles.map((item) => ({ value: item.id, label: item.name }))} onChange={setId} />
      <Input value={channel} onChange={(event) => setChannel(event.target.value)} />
      <Input value={text} onChange={(event) => setText(event.target.value)} />
      <Space wrap>
        <Input value={ttlKey} onChange={(event) => setTtlKey(event.target.value)} placeholder="键" />
        <InputNumber value={ttl} onChange={(value) => setTtl(value ?? 60)} />
        <Button onClick={() => id && void api.redisExpire(id, ttlKey, ttl).then((note) => message.success(note)).catch((err) => message.error(errorText(err)))}>设置 TTL</Button>
      </Space>
      <Space>
        <Button type="primary" onClick={() => id && void api.redisPublish(id, channel, text).then((count) => message.success(`已送达 ${count} 个订阅者`)).catch((err) => message.error(errorText(err)))}>发布</Button>
        <Button onClick={() => setListening((value) => !value)} disabled={!id}>{listening ? "停止监听" : "持续监听"}</Button>
      </Space>
      <Input.TextArea rows={6} value={log.join("\n")} readOnly />
    </div>
  );
}

export function MongoBuilder() {
  const { message } = App.useApp();
  const profiles = useProfiles().filter((item) => item.engine === "mongodb");
  const openQuery = useTabStore((state) => state.openQuery);
  const [id, setId] = useState<string>();
  const [collection, setCollection] = useState("books");
  const [collections, setCollections] = useState<string[]>([]);
  const [filter, setFilter] = useState('{ "price": { "$gt": 10 } }');
  const [pipeline, setPipeline] = useState('[{ "$match": { "price": { "$gt": 10 } } }, { "$limit": 20 }]');
  const profile = profiles.find((item) => item.id === id);

  useEffect(() => {
    if (!id || !profile) return;
    const database = profile.database || "";
    void api.listTables(id, database, database).then(setCollections).catch((err) => message.error(errorText(err)));
  }, [id, profile, message]);

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="info" showIcon message="生成的 find / aggregate 会在查询页执行。集合树就是左侧对象浏览器里的集合；结构分析在“生成、脱敏与字典”。" />
      <Select style={{ minWidth: 200 }} placeholder="MongoDB" value={id} options={profiles.map((item) => ({ value: item.id, label: item.name }))} onChange={setId} />
      {collections.length > 0 ? (
        <Select showSearch style={{ minWidth: 220 }} value={collection} options={collections.map((name) => ({ value: name, label: name }))} onChange={setCollection} />
      ) : (
        <Input value={collection} onChange={(event) => setCollection(event.target.value)} placeholder="集合" />
      )}
      <Input.TextArea rows={3} value={filter} onChange={(event) => setFilter(event.target.value)} />
      <Button onClick={() => openQuery({ connectionId: id, database: profile?.database || "", sql: `find ${collection} ${filter}`, title: `find ${collection}` })}>打开 find</Button>
      <Input.TextArea rows={4} value={pipeline} onChange={(event) => setPipeline(event.target.value)} />
      <Button type="primary" onClick={() => {
        try {
          JSON.parse(pipeline);
          openQuery({ connectionId: id, database: profile?.database || "", sql: `aggregate ${collection} ${pipeline}`, title: `aggregate ${collection}` });
        } catch (err) {
          message.error(errorText(err));
        }
      }}>打开 aggregate</Button>
    </div>
  );
}

export function DebugPanel() {
  const { message } = App.useApp();
  const profiles = useProfiles();
  const [id, setId] = useState<string>();
  const [database, setDatabase] = useState("");
  const [fn, setFn] = useState("public.fn");
  const [callSql, setCallSql] = useState("SELECT public.fn()");
  const [line, setLine] = useState(1);
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<{ message: string; steps: string[]; available: boolean } | null>(null);
  const [snap, setSnap] = useState<Awaited<ReturnType<typeof api.debugLaunch>> | null>(null);
  async function run(action: () => Promise<Awaited<ReturnType<typeof api.debugLaunch>>>) {
    setBusy(true);
    try {
      setSnap(await action());
    } catch (err) {
      message.error(errorText(err));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="warning" showIcon message="PostgreSQL 在已安装 pldbgapi 时用两条连接调试：控制会话下断点并单步，目标会话执行调用并等待。没有扩展时只显示探测结果。MySQL 和 Oracle 没有调试协议。" />
      <Select style={{ minWidth: 200 }} placeholder="连接" value={id} options={profiles.map((item) => ({ value: item.id, label: item.name }))} onChange={(value) => { setId(value); setDatabase(defaultDb(profiles, value)); }} />
      <Space wrap>
        <Button onClick={() => id && void api.debugStatus(id, database).then(setReport).catch((err) => message.error(errorText(err)))}>探测</Button>
        <Input style={{ width: 180 }} value={fn} onChange={(event) => setFn(event.target.value)} placeholder="schema.函数" />
        <InputNumber value={line} onChange={(value) => setLine(Number(value ?? 1))} />
      </Space>
      <Input.TextArea rows={3} value={callSql} onChange={(event) => setCallSql(event.target.value)} placeholder="目标会话执行的 SELECT 函数调用" />
      <Space wrap>
        <Button type="primary" loading={busy} disabled={!id} onClick={() => id && void run(() => api.debugLaunch(id, database, fn, callSql, line))}>开始</Button>
        <Button loading={busy} disabled={!id} onClick={() => id && void run(() => api.debugStep(id, "step_over"))}>步过</Button>
        <Button loading={busy} disabled={!id} onClick={() => id && void run(() => api.debugStep(id, "step_into"))}>步入</Button>
        <Button loading={busy} disabled={!id} onClick={() => id && void run(() => api.debugStep(id, "continue"))}>继续</Button>
        <Button loading={busy} disabled={!id} onClick={() => id && void run(() => api.debugStep(id, "wait"))}>等待</Button>
        <Button danger loading={busy} disabled={!id} onClick={() => id && void run(() => api.debugStop(id))}>中止</Button>
      </Space>
      {report && (
        <>
          <Alert type={report.available ? "success" : "info"} showIcon message={report.message} />
          <Input.TextArea rows={4} value={report.steps.join("\n\n")} readOnly />
        </>
      )}
      {snap && (
        <>
          <Alert type={snap.status === "paused" ? "success" : snap.status === "timeout" ? "warning" : "info"} showIcon message={`${snap.status}${snap.line != null ? ` · 行 ${snap.line}` : ""}`} description={snap.message} />
          {snap.targetNote && <div className="text-sm text-[var(--xb-muted)]">{snap.targetNote}</div>}
          <Table size="small" pagination={false} rowKey={(row) => `${row.name}-${row.detail}`} dataSource={snap.variables} columns={[{ title: "变量", dataIndex: "name" }, { title: "值", dataIndex: "value" }, { title: "详情", dataIndex: "detail" }]} locale={{ emptyText: "没有变量" }} />
          <Table size="small" pagination={false} rowKey={(row) => `${row.name}-${row.value}-${row.detail}`} dataSource={snap.stack} columns={[{ title: "栈", dataIndex: "name" }, { title: "行", dataIndex: "value" }, { title: "详情", dataIndex: "detail" }]} locale={{ emptyText: "没有栈" }} />
        </>
      )}
    </div>
  );
}
