import { Alert, Button, Table, Tag } from "antd";
import { useEffect, useState } from "react";
import { api, errorText } from "../api";
import { useTabStore } from "../stores/useTabStore";
import type { ColumnMeta, StructureTab } from "../types";

export function StructurePanel({ tab }: { tab: StructureTab }) {
  const openData = useTabStore((state) => state.openData);
  const [columns, setColumns] = useState<ColumnMeta[]>([]);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    api
      .listColumns(tab.connectionId, tab.database, tab.schema, tab.table)
      .then((rows) => {
        if (!cancelled) setColumns(rows);
      })
      .catch((err) => {
        if (!cancelled) setError(errorText(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [tab.connectionId, tab.database, tab.schema, tab.table]);

  return (
    <div className="xb-panel">
      <div className="flex items-center justify-between">
        <div>
          <div className="text-sm font-medium">{tab.table}</div>
          <div className="text-xs text-[var(--xb-muted)]">
            {[tab.database, tab.schema].filter(Boolean).join(" / ") || "对象结构"}
          </div>
        </div>
        <Button
          onClick={() =>
            openData({
              connectionId: tab.connectionId,
              database: tab.database,
              schema: tab.schema,
              table: tab.table,
              readOnly: false,
              title: tab.table,
            })
          }
        >
          打开数据
        </Button>
      </div>
      {error && <Alert type="error" showIcon message={error} />}
      <div className="xb-grid-wrap">
        <Table
          size="small"
          rowKey="name"
          loading={loading}
          pagination={false}
          dataSource={columns}
          columns={[
            { title: "列", dataIndex: "name" },
            { title: "类型", dataIndex: "dataType" },
            {
              title: "空",
              dataIndex: "nullable",
              render: (value: boolean) => (value ? "是" : "否"),
            },
            {
              title: "键",
              dataIndex: "primaryKey",
              render: (value: boolean) => (value ? <Tag color="gold">主键</Tag> : ""),
            },
            {
              title: "自增",
              dataIndex: "autoIncrement",
              render: (value: boolean) => (value ? "是" : ""),
            },
            {
              title: "默认值",
              dataIndex: "defaultValue",
              render: (value?: string | null) => value || "",
            },
          ]}
        />
      </div>
    </div>
  );
}
