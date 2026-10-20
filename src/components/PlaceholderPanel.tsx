import { Button, Tag } from "antd";
import type { PlaceholderTab } from "../types";

export function PlaceholderPanel({ tab }: { tab: PlaceholderTab }) {
  return (
    <div className="xb-panel">
      <div className="xb-welcome-card">
        <Tag color="gold">开发中</Tag>
        <h2 className="mb-2 mt-3 text-xl">{tab.title}</h2>
        <p className="text-[var(--xb-muted)]">{tab.summary}</p>
        <ul className="mt-3 pl-5">
          {tab.points.map((point) => (
            <li key={point}>{point}</li>
          ))}
        </ul>
        <Button disabled className="mt-2">
          开始使用
        </Button>
      </div>
    </div>
  );
}
