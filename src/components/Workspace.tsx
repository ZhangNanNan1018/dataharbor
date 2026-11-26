import { PanelBoundary } from "./PanelBoundary";
import { App, Alert, Dropdown, Tabs } from "antd";
import type { MenuProps } from "antd";
import { lazy, memo, Suspense, useRef } from "react";
import { errorText } from "../api";
import { openSampleBookstore } from "../actions";
import { useTabStore } from "../stores/useTabStore";
import { cancelTabOperations, OperationOwnerContext, useOperationStore } from "../operations";
import type { WorkspaceTab } from "../types";
import { DataGridPanel } from "./DataGridPanel";
import { StudioPanel } from "./StudioPanel";
import { StructurePanel } from "./StructurePanel";
import { Welcome } from "./Welcome";
import { closeWithTabMotion } from "./tabMotion";

const QueryPanel = lazy(() => import("./QueryPanel").then((module) => ({ default: module.QueryPanel })));

export function Workspace() {
  const { message, modal } = App.useApp();
  const tabs = useTabStore((state) => state.tabs);
  const activeId = useTabStore((state) => state.activeId);
  const dirty = useTabStore((state) => state.dirty);
  const draftSaveError = useTabStore((state) => state.draftSaveError);
  const close = useTabStore((state) => state.close);
  const setActive = useTabStore((state) => state.setActive);
  const workspace = useRef<HTMLElement>(null);

  async function openSample() {
    try {
      await openSampleBookstore();
      message.success("示例书店已就绪");
    } catch (err) {
      message.error(errorText(err));
    }
  }

  function requestClose(targets: WorkspaceTab[], anchorId: string) {
    if (!targets.length) return;
    const ids = targets.map((tab) => tab.id);
    const finish = () => closeWithTabMotion(workspace.current, ids, () => close(ids, anchorId));
    const running = () => useOperationStore.getState().active.filter((op) => op.ownerId && ids.includes(op.ownerId));
    const busy = running().length > 0;
    const queries = targets.some((tab) => tab.kind === "query");
    const edits = targets.some((tab) => tab.kind !== "query" && useTabStore.getState().dirty[tab.id]);
    if (!busy && !queries && !edits) {
      finish();
      return;
    }
    const title = targets.length > 1 ? `关闭这 ${targets.length} 个标签？`
      : busy ? "取消运行中的任务并关闭标签？" : queries ? "关闭查询草稿？" : "关闭未提交的标签？";
    modal.confirm({
      title,
      content: [
        busy && "将取消任务并等待后台停止。已经提交的数据会保留。",
        queries && "关闭查询会移除 SQL 草稿及其恢复记录，断开标签会话，未提交的事务将由数据库回滚。",
        edits && "未提交的修改会丢失。",
      ].filter(Boolean).join(" "),
      okText: busy ? "取消任务并等待关闭" : "关闭",
      cancelText: busy ? "继续查看" : "留下",
      onOk: async () => {
        try {
          if (busy) await Promise.all(ids.map(cancelTabOperations));
          if (running().length) throw new Error("标签有新任务正在运行，请重新确认关闭。");
          finish();
        } catch (error) {
          message.error(errorText(error));
          throw error;
        }
      },
    });
  }

  function tabMenu(id: string): MenuProps {
    const index = tabs.findIndex((tab) => tab.id === id);
    const items: MenuProps["items"] = [
      { key: "close", label: "关闭此标签" },
      { key: "others", label: "关闭其他标签", disabled: tabs.length === 1 },
      { key: "left", label: "关闭左侧标签", disabled: index === 0 },
      { key: "right", label: "关闭右侧标签", disabled: index === tabs.length - 1 },
      { key: "all", label: "关闭全部标签" },
    ];
    if (tabs[index]?.kind === "query") {
      items.unshift({ key: "duplicate", label: "复制查询标签" }, { type: "divider" });
    }
    return { items, onClick: ({ key }) => {
      const state = useTabStore.getState();
      const i = state.tabs.findIndex((tab) => tab.id === id);
      if (i < 0) return;
      const tab = state.tabs[i];
      switch (key) {
        case "duplicate":
          if (tab.kind === "query") state.openQuery({ ...tab, title: `${tab.title} · 副本` });
          break;
        case "close": requestClose([tab], id); break;
        case "others": requestClose(state.tabs.filter((item) => item.id !== id), id); break;
        case "left": requestClose(state.tabs.slice(0, i), id); break;
        case "right": requestClose(state.tabs.slice(i + 1), id); break;
        case "all": requestClose(state.tabs, id); break;
      }
    } };
  }

  if (tabs.length === 0) {
    return (
      <section ref={workspace} className="xb-workspace">
        {draftSaveError && <Alert type="warning" showIcon message={draftSaveError} />}
        <Welcome onSample={() => void openSample()} />
      </section>
    );
  }

  return (
    <section ref={workspace} className="xb-workspace">
      {draftSaveError && <Alert type="warning" showIcon message={draftSaveError} />}
      <Tabs
        className="xb-tabs"
        type="editable-card"
        hideAdd
        destroyOnHidden={false}
        activeKey={activeId ?? undefined}
        onChange={setActive}
        renderTabBar={(props, TabBar) => (
          <TabBar {...props}>
            {(node) => (
              <Dropdown trigger={["contextMenu"]} menu={tabMenu(String(node.key))}>
                {node}
              </Dropdown>
            )}
          </TabBar>
        )}
        onEdit={(target, action) => {
          if (action !== "remove" || typeof target !== "string") return;
          const tab = tabs.find((item) => item.id === target);
          if (tab) requestClose([tab], target);
        }}
        items={tabs.map((tab) => ({
          key: tab.id,
          label: <span className="xb-tab-title">{tab.title}{dirty[tab.id] ? " *" : ""}</span>,
          children: <OperationOwnerContext.Provider value={tab.id}><PanelBoundary ownerId={tab.id}><TabContent tab={tab} /></PanelBoundary></OperationOwnerContext.Provider>,
        }))}
      />
    </section>
  );
}

const TabContent = memo(function TabContent({ tab }: { tab: WorkspaceTab }) {
  switch (tab.kind) {
    case "query":
      return <Suspense fallback={<div className="p-3 text-xs">正在加载查询编辑器…</div>}><QueryPanel tab={tab} /></Suspense>;
    case "data":
      return <DataGridPanel tab={tab} />;
    case "structure":
      return <StructurePanel tab={tab} />;
    case "placeholder":
      return <StudioPanel tab={tab} />;
  }
});
