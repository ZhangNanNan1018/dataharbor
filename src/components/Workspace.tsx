import { App, Tabs } from "antd";
import { errorText } from "../api";
import { openSampleBookstore } from "../actions";
import { useTabStore } from "../stores/useTabStore";
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
        <Welcome onSample={() => void openSample()} />
      </section>
    );
  }

  return (
    <section className="xb-workspace">
      <Tabs
        className="xb-tabs"
        type="editable-card"
        hideAdd
        destroyOnHidden={false}
        activeKey={activeId ?? undefined}
        onChange={setActive}
        onEdit={(target, action) => {
          if (action !== "remove" || typeof target !== "string") return;
          if (dirty[target]) {
            modal.confirm({
              title: "关闭未提交的标签？",
              content: "未提交的修改会丢失。",
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
          children: renderTab(tab),
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
