# 模块边界

React 界面经 `src/api.ts` 调用 Tauri 命令；`commands.rs` 转交业务模块。`state.rs` 管理连接、会话和运行时隧道，`drivers/` 实现各数据库协议。原有命令名称保持兼容。

## Rust 工作台

`src-tauri/src/studio.rs` 只声明领域模块并导出命令使用的接口。领域模块显式导入依赖，内部辅助项使用 `pub(super)`，不靠通配导入共享全部实现。

| 模块 | 负责内容 |
| --- | --- |
| `studio/types.rs` | 工作台请求、响应和持久数据契约 |
| `studio/storage.rs` | 工作台本地文件及通用值转换 |
| `studio/table_data.rs` | 表数据完整读取、元数据匹配、稳定顺序和写入批次 |
| `studio/sync.rs` | 结构差异选择、主键对比与同步保护 |
| `studio/file_io.rs` | 文件格式导入导出 |
| `studio/backup.rs` | 原生工具备份、受限数据脚本、还原入口 |
| `studio/catalog.rs` | 目录、索引、外键、ER 和字典 |
| `studio/inspection.rs` | 结构分析和表概况 |
| `studio/engine_tools.rs` | Redis 工具与引擎调试能力检测 |
| `studio/modeling.rs` | ER、模型和看板存储 |
| `studio/ai.rs` | AI 设置、请求与数据发送 |
| `studio/automation.rs` | 定时任务、邮件与系统计划入口 |
| `studio/project.rs` | 项目包、片段和收藏，导入任务禁用规则 |
| `studio/query_tools.rs` | SQL 格式、参数和对象设计辅助 |

`sqlutil/pagination.rs` 集中处理分页方言，`sqlutil/scan.rs` 为分句和读取分类提供共同词法边界。网格、查询、CSV 和工作台复用规则；显式限制、锁定查询和带写操作的 CTE 不自动重写。任意用户查询不会被额外重放进行 COUNT，分页使用下一行探测；失败不会自动重试原语句。

`DbSession::execute_transaction` 是批量 SQL 修改的事务边界。调用前完整编译和校验，默认实现保持同一会话；SQL Server 使用其事务语法，Oracle 覆写为一次 SQL*Plus 原子脚本，不能提供持久事务的 Snowflake 路径明确拒绝。

同步只用真实完整主键，复合键编码保持值边界和类型。全部读取及检查通过后才开始写入。每批独立事务，不保证整个多批操作原子性；操作期间应停止源和目标的其他写入。

## React 工作台

`src/components/StudioPanel.tsx` 只按工具类型分派，实现在 `src/components/studio/`：查询设计、建模、同步、文件操作、分析、任务、AI、项目包、目录和引擎工具各自维护。`shared.tsx` 仅包含共有连接字段、结果网格、操作锁及确认交互。

请求契约在 `src/api.ts` 具名声明。`FileExportResult.kind/note` 区分普通导出、原生备份、SQLite 表脚本和纯数据脚本；界面按真实结果展示完成、警告或失败。
