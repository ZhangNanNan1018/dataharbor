export interface PlaceholderSpec {
  id: string;
  title: string;
  summary: string;
  points: string[];
}

export const PLACEHOLDERS: Record<string, PlaceholderSpec> = {
  builder: { id: "builder", title: "查询构建器", summary: "", points: [] },
  designer: { id: "designer", title: "对象设计器", summary: "", points: [] },
  er: { id: "er", title: "ER 图", summary: "", points: [] },
  "schema-sync": { id: "schema-sync", title: "结构同步", summary: "", points: [] },
  "data-sync": { id: "data-sync", title: "数据同步", summary: "", points: [] },
  io: { id: "io", title: "导入导出", summary: "", points: [] },
  backup: { id: "backup", title: "备份与还原", summary: "", points: [] },
  generate: { id: "generate", title: "生成与脱敏", summary: "", points: [] },
  bi: { id: "bi", title: "BI 看板", summary: "", points: [] },
  model: { id: "model", title: "数据模型", summary: "", points: [] },
  jobs: { id: "jobs", title: "定时任务", summary: "", points: [] },
  users: { id: "users", title: "用户与监控", summary: "", points: [] },
  ai: { id: "ai", title: "AI 助手", summary: "", points: [] },
  collab: { id: "collab", title: "项目与收藏", summary: "", points: [] },
  redis: { id: "redis", title: "Redis 发布订阅", summary: "", points: [] },
  search: { id: "search", title: "库内搜索", summary: "", points: [] },
  mongo: { id: "mongo", title: "Mongo 查询构建", summary: "", points: [] },
  debug: { id: "debug", title: "过程调试", summary: "", points: [] },
};

export function placeholderById(id: string): PlaceholderSpec {
  return (
    PLACEHOLDERS[id] ?? {
      id,
      title: "开发中",
      summary: "这个能力还在路线图里。",
      points: [],
    }
  );
}
