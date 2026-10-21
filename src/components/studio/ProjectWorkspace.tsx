import { App, Alert, Button, Input, Select, Space } from "antd";
import { useContext, useEffect, useState } from "react";
import { api, errorText, pickOpenPath, pickSavePath, type Favorite, type Snippet } from "../../api";
import { useTabStore } from "../../stores/useTabStore";
import { useConnectionStore } from "../../stores/useConnectionStore";
import { StudioSeedContext, useOperation, useProfiles } from "./shared";

export function Collab() {
  const { message } = App.useApp();
  const profiles = useProfiles();
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [favorites, setFavorites] = useState<Favorite[]>([]);
  const [name, setName] = useState("常用查询");
  const [body, setBody] = useState("SELECT * FROM books");
  const openQuery = useTabStore((state) => state.openQuery);
  const loadConnections = useConnectionStore((state) => state.load);
  const { busy, run } = useOperation();
  const [projectNote, setProjectNote] = useState("");
  const seed = useContext(StudioSeedContext);
  const [favoriteId, setFavoriteId] = useState<string>();

  useEffect(() => {
    if (seed?.connectionId) setFavoriteId(seed.connectionId);
  }, [seed?.connectionId]);

  async function reload() {
    setSnippets(await api.listSnippets());
    setFavorites(await api.listFavorites());
  }
  useEffect(() => { void reload().catch(() => undefined); }, []);

  return (
    <div className="xb-panel space-y-3 overflow-auto p-3">
      <Alert type="warning" showIcon message="项目包含连接地址、账号、数据库名、SQL 片段、任务、看板、模型和收藏；密码仍是密文，但其他内容可能敏感，请勿直接公开。导入的新增任务默认禁用，请检查后手动启用。" />
      <Space>
        <Button disabled={!!busy} loading={busy === "export"} onClick={() => void run("export", async () => {
          const path = await pickSavePath("dataharbor-project.json");
          if (!path) return;
          setProjectNote("");
          setProjectNote(await api.exportProject(path));
        })}>导出项目包</Button>
        <Button disabled={!!busy} loading={busy === "import"} onClick={() => void run("import", async () => {
          const path = await pickOpenPath();
          if (!path) return;
          setProjectNote("");
          setProjectNote(await api.importProject(path));
          await loadConnections();
          await reload();
        })}>导入项目包</Button>
      </Space>
      {projectNote && <Alert type="info" showIcon message={projectNote} />}
      <Space wrap>
        <Input value={name} onChange={(event) => setName(event.target.value)} />
        <Button onClick={() => void api.saveSnippet({ name, body, engine: "sqlite" }).then(reload).catch((err) => message.error(errorText(err)))}>保存片段</Button>
      </Space>
      <Input.TextArea rows={3} value={body} onChange={(event) => setBody(event.target.value)} />
      {snippets.map((snippet) => (
        <div key={snippet.id} className="flex items-center gap-2 text-sm">
          <span>{snippet.name}</span>
          <Button size="small" onClick={() => openQuery({ sql: snippet.body, title: snippet.name })}>打开</Button>
          <Button size="small" danger onClick={() => void api.deleteSnippet(snippet.id).then(reload)}>删除</Button>
        </div>
      ))}
      <Space wrap>
        <Select
          style={{ minWidth: 180 }}
          placeholder="要收藏的连接"
          value={favoriteId}
          options={profiles.map((item) => ({ value: item.id, label: item.name }))}
          onChange={setFavoriteId}
        />
        <Button disabled={!favoriteId} onClick={() => {
          const profile = profiles.find((item) => item.id === favoriteId);
          if (!profile) return;
          const same = seed?.connectionId === profile.id;
          void api.saveFavorite({
            connectionId: profile.id,
            database: same ? seed?.database || profile.database : profile.database,
            schema: same ? seed?.schema || "" : "",
            objectName: same && seed?.table ? seed.table : profile.name,
            kind: same && seed?.table ? "table" : "connection",
          }).then(reload).catch((err) => message.error(errorText(err)));
        }}>{seed?.table && seed.connectionId === favoriteId ? `收藏 ${seed.table}` : "收藏连接"}</Button>
      </Space>
      {favorites.map((item) => <div key={item.id} className="text-xs">{item.objectName}</div>)}
    </div>
  );
}
