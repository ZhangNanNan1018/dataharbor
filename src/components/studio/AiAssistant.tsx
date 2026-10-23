import { App, Alert, Button, Input, Select, Space } from "antd";
import { useEffect, useState } from "react";
import { api } from "../../api";
import { ENGINES } from "../../engines";
import { useOperation } from "./shared";

export function AiAssistant() {
  const { message } = App.useApp();
  const [endpoint, setEndpoint] = useState("");
  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [savedEndpoint, setSavedEndpoint] = useState("");
  const [savedModel, setSavedModel] = useState("");
  const [task, setTask] = useState("explain");
  const [sql, setSql] = useState("SELECT * FROM books");
  const [engine, setEngine] = useState("sqlite");
  const [target, setTarget] = useState("postgres");
  const [schemaText, setSchemaText] = useState("books(id, title, price)");
  const [prompt, setPrompt] = useState("这个查询会扫全表吗？");
  const [answer, setAnswer] = useState("");
  const [rooms, setRooms] = useState<{ id: string; title: string; messages: string[] }[]>(() => readRooms());
  const [roomId, setRoomId] = useState("");
  const [compareModel, setCompareModel] = useState("");
  const { busy, run } = useOperation();
  const serviceChanged = hasKey && !!savedEndpoint && serviceOrigin(endpoint) !== serviceOrigin(savedEndpoint);
  const settingsChanged = endpoint.trim() !== savedEndpoint || model.trim() !== savedModel || !!apiKey;

  useEffect(() => {
    void api.aiSettings().then((settings) => {
      setEndpoint(settings.endpoint);
      setSavedEndpoint(settings.endpoint);
      setModel(settings.model);
      setSavedModel(settings.model);
      setHasKey(settings.hasKey);
    }).catch(() => undefined);
  }, []);

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="info" showIcon message={hasKey ? "已保存密钥。请求发往你填写的兼容接口。" : "填写自己的 OpenAI 兼容接口和密钥。没有密钥时不会编造回复。"} />
      <Alert type="warning" showIcon message="发送会把 SQL、结构说明、提示词或用于补全的单元格内容发送至已保存的接口。请先确认这些内容允许提供给该服务。" />
      <span className="text-xs text-[var(--xb-muted)]">远程服务请使用 HTTPS；本机 localhost、127.0.0.1 或 ::1 可使用 HTTP。</span>
      {serviceChanged && <Alert type="warning" showIcon message="服务地址已更换，请填写新服务的 API 密钥；旧密钥不会发送给新服务。" />}
      {settingsChanged && <Alert type="info" showIcon message="接口设置尚未保存，保存后才能发送。" />}
      <Input aria-label="AI 服务地址" disabled={!!busy} value={endpoint} onChange={(event) => setEndpoint(event.target.value)} placeholder="https://api.openai.com/v1/chat/completions" />
      <Space wrap>
        <Input disabled={!!busy} value={model} onChange={(event) => setModel(event.target.value)} placeholder="模型" />
        <Input.Password aria-label="API 密钥" disabled={!!busy} value={apiKey} onChange={(event) => setApiKey(event.target.value)} placeholder="API 密钥" />
        <Button disabled={!!busy} loading={busy === "save"} onClick={() => void run("save", async () => {
          if (serviceChanged && !apiKey.trim()) throw new Error("更换服务地址后，请重新填写该服务的 API 密钥");
          const settings = await api.saveAiSettings({ endpoint, model, apiKey, clearKey: false });
          setEndpoint(settings.endpoint); setSavedEndpoint(settings.endpoint);
          setModel(settings.model); setSavedModel(settings.model);
          setHasKey(settings.hasKey); setApiKey(""); message.success("已保存");
        })}>保存</Button>
      </Space>
      <Select value={task} style={{ width: 160 }} options={[
        { value: "explain", label: "解释" },
        { value: "optimize", label: "优化" },
        { value: "format", label: "格式化" },
        { value: "convert", label: "转换方言" },
        { value: "fix", label: "修复" },
        { value: "chat", label: "对话" },
      ]} onChange={setTask} />
      <Input.TextArea rows={4} value={sql} onChange={(event) => setSql(event.target.value)} />
      <Space>
        <Select value={engine} style={{ width: 140 }} options={ENGINES.map((item) => ({ value: item.value, label: item.label }))} onChange={setEngine} />
        <Select value={target} style={{ width: 140 }} options={ENGINES.map((item) => ({ value: item.value, label: item.label }))} onChange={setTarget} />
      </Space>
      <Input value={schemaText} onChange={(event) => setSchemaText(event.target.value)} placeholder="附上的结构说明" />
      <Input value={prompt} onChange={(event) => setPrompt(event.target.value)} />
      <Space wrap>
        <Button type="primary" disabled={!!busy || settingsChanged} loading={busy === "send"} onClick={() => void run("send", async () => {
          const text = await api.aiAsk({ task, sql, engine, schemaText, prompt, targetEngine: target, model });
          setAnswer(text);
          const current = roomId || crypto.randomUUID();
          const next = upsertRoom(rooms, current, text);
          setRoomId(current);
          setRooms(next);
          localStorage.setItem("xb-ai-rooms", JSON.stringify(next));
        })}>发送</Button>
        <Button disabled={!!busy} onClick={() => {
          const source = rooms.find((room) => room.id === roomId);
          const copy = { id: crypto.randomUUID(), title: `${source?.title ?? "对话"} 的分支`, messages: [...(source?.messages ?? [])] };
          const next = [copy, ...rooms];
          setRooms(next);
          setRoomId(copy.id);
          localStorage.setItem("xb-ai-rooms", JSON.stringify(next));
        }}>分叉当前房间</Button>
        <Input style={{ width: 180 }} value={compareModel} onChange={(event) => setCompareModel(event.target.value)} placeholder="对照模型" />
        <Button disabled={!!busy || settingsChanged} loading={busy === "compare"} onClick={() => void run("compare", async () => {
          const first = await api.aiAsk({ task, sql, engine, schemaText, prompt, targetEngine: target, model });
          const second = compareModel.trim() ? await api.aiAsk({ task, sql, engine, schemaText, prompt, targetEngine: target, model: compareModel.trim() }) : "";
          setAnswer(second ? `【${model || "已保存模型"}】\n${first}\n\n【${compareModel}】\n${second}` : first);
        })}>对照两个模型</Button>
      </Space>
      <Select
        disabled={!!busy}
        style={{ minWidth: 220 }}
        placeholder="对话房间"
        value={roomId || undefined}
        options={rooms.map((room) => ({ value: room.id, label: room.title }))}
        onChange={(value) => {
          setRoomId(value);
          const room = rooms.find((item) => item.id === value);
          setAnswer(room?.messages.join("\n\n---\n\n") ?? "");
        }}
      />
      <Input.TextArea rows={10} value={answer} readOnly />
    </div>
  );
}

function serviceOrigin(endpoint: string): string {
  try { return new URL(endpoint.trim()).origin; } catch { return endpoint.trim(); }
}

function readRooms(): { id: string; title: string; messages: string[] }[] {
  try {
    const parsed = JSON.parse(localStorage.getItem("xb-ai-rooms") || "[]") as { id: string; title: string; messages: string[] }[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

function upsertRoom(rooms: { id: string; title: string; messages: string[] }[], id: string, text: string) {
  const existing = rooms.find((room) => room.id === id);
  if (!existing) return [{ id, title: `房间 ${rooms.length + 1}`, messages: [text] }, ...rooms].slice(0, 20);
  return rooms.map((room) => room.id === id ? { ...room, messages: [...room.messages, text].slice(-30) } : room);
}
