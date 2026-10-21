import { App, Button } from "antd";
import { api, errorText } from "../api";
import { engineLabel } from "../engines";
import { hasRecent, recentRank, touchRecent } from "../recent";
import { useConnectionStore } from "../stores/useConnectionStore";
import { useTabStore } from "../stores/useTabStore";
import { useUiStore } from "../stores/useUiStore";
import { EngineGlyph } from "./EngineGlyph";
import { Logo } from "./Logo";

export function Welcome({ onSample }: { onSample: () => void }) {
  const { message } = App.useApp();
  const openConnection = useUiStore((state) => state.openConnection);
  const openPalette = useUiStore((state) => state.openPalette);
  const profiles = useConnectionStore((state) => state.profiles);
  const markConnected = useConnectionStore((state) => state.markConnected);
  const setStatus = useUiStore((state) => state.setStatus);
  const openQuery = useTabStore((state) => state.openQuery);
  const recent = [...profiles].sort((a, b) => recentRank(a.id) - recentRank(b.id) || Number(b.connected) - Number(a.connected)).slice(0, 6);

  return (
    <div className="xb-welcome">
      <div className="xb-welcome-inner">
        <Logo size={36} />
        <div className="xb-kicker mt-4">DataHarbor</div>
        <h1>把库、表和 SQL 放在同一张工作台</h1>
        <p className="lead">
          连接关系库和文档库，浏览对象，运行查询，在表格里改数据，再提交或放弃。Ctrl/Cmd+K 打开命令，Ctrl/Cmd+P 打开任意表。
        </p>
        <div className="xb-action-row">
          <Button type="primary" onClick={() => openConnection(null)}>
            新建连接
          </Button>
          <Button onClick={onSample}>打开示例书店</Button>
          <Button onClick={() => openPalette("command")}>命令面板</Button>
        </div>
        <div className="xb-section-label">{hasRecent() ? "最近的连接" : "连接"}</div>
        {recent.length === 0 ? (
          <p className="lead">还没有保存的连接。新建一个，或先打开示例书店。</p>
        ) : (
          <div className="xb-conn-grid">
            {recent.map((profile) => (
              <article key={profile.id} className="xb-conn-card">
                <header>
                  <EngineGlyph engine={profile.engine} />
                  <div>
                    <strong>{profile.name}</strong>
                    <small>
                      {engineLabel(profile.engine)}
                      {profile.connected ? " · 在线" : ""}
                    </small>
                  </div>
                </header>
                <div className="flex gap-2">
                  <Button
                    size="small"
                    type="primary"
                    onClick={() => {
                      void api
                        .connect(profile.id)
                        .then((result) => {
                          markConnected(profile.id, true);
                          touchRecent(profile.id);
                          setStatus(`已连接 ${profile.name} · ${result.version}`);
                          openQuery({
                            connectionId: profile.id,
                            database: profile.engine === "sqlite" ? "main" : profile.database,
                            title: `查询 · ${profile.name}`,
                          });
                        })
                        .catch((err) => message.error(errorText(err)));
                    }}
                  >
                    连接
                  </Button>
                  <Button size="small" onClick={() => openConnection(profile.id)}>
                    编辑
                  </Button>
                </div>
              </article>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
