import { App, Alert, Tabs } from "antd";
import { errorText } from "../api";
import { openSampleBookstore } from "../actions";
import { useTabStore } from "../stores/useTabStore";
import { cancelTabOperations, OperationOwnerContext, useOperationStore } from "../operations";
import type { WorkspaceTab } from "../types";
import { DataGridPanel } from "./DataGridPanel";
import { StudioPanel } from "./StudioPanel";
import { QueryPanel } from "./QueryPanel";
import { StructurePanel } from "./StructurePanel";
import { Welcome } from "./Welcome";

export function Workspace() {
  const { message, modal } = App.useApp();
  const tabs = useTabStore((state) => state.tabs);
  const activeId = useTabStore((state) => state.activeId);
  const dirty = useTabStore((state) => state.dirty);
  const draftSaveError = useTabStore((state) => state.draftSaveError);
  const close = useTabStore((state) => state.close);
  const setActive = useTabStore((state) => state.setActive);

  async function openSample() {
    try {
      await openSampleBookstore();
      message.success("示例书店已就绪");
    } catch (err) {
      message.error(errorText(err));
    }
  }

  if (tabs.length === 0) {
    return (
      <section className="xb-workspace">
        {draftSaveError && <Alert type="warning" showIcon message={draftSaveError} />}
        <Welcome onSample={() => void openSample()} />
      </section>
    );
  }

  return (
    <section className="xb-workspace">
      {draftSaveError && <Alert type="warning" showIcon message={draftSaveError} />}
      <Tabs
        className="xb-tabs"
        type="editable-card"
        hideAdd
        destroyOnHidden={false}
        activeKey={activeId ?? undefined}
        onChange={setActive}
        onEdit={(target, action) => {
          if (action !== "remove" || typeof target !== "string") return;
          if (useOperationStore.getState().active.some((operation) => operation.ownerId === target)) {
            modal.confirm({
              title: "取消运行中的任务并关闭标签？",
              content: `将请求取消并等待后台实际停止。当前批次可能仍会完成，已提交的数据不会撤销。${dirty[target] ? "关闭后未保存的草稿也会丢失。" : ""}`,
              okText: "取消任务并等待关闭",
              cancelText: "继续查看",
              onOk: async () => {
                try { await cancelTabOperations(target); close(target); }
                catch (error) { message.error(errorText(error)); throw error; }
              },
            });
            return;
          }
          if (dirty[target]) {
            const query = tabs.find((tab) => tab.id === target)?.kind === "query";
            modal.confirm({
              title: query ? "关闭查询草稿？" : "关闭未提交的标签？",
              content: query ? "关闭后将移除这份 SQL 草稿及其自动恢复记录。" : "未提交的修改会丢失。",
              okText: "关闭",
              cancelText: "留下",
              onOk: () => close(target),
            });
            return;
          }
          close(target);
        }}
        items={tabs.map((tab) => ({
          key: tab.id,
          label: dirty[tab.id] ? `${tab.title} *` : tab.title,
          children: <OperationOwnerContext.Provider value={tab.id}>{renderTab(tab)}</OperationOwnerContext.Provider>,
        }))}
      />
    </section>
  );
}

function renderTab(tab: WorkspaceTab) {
  switch (tab.kind) {
    case "query":
      return <QueryPanel tab={tab} />;
    case "data":
      return <DataGridPanel tab={tab} />;
    case "structure":
      return <StructurePanel tab={tab} />;
    case "placeholder":
      return <StudioPanel tab={tab} />;
  }
}
