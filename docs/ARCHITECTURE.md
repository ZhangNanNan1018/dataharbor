# 模块边界

React 界面经 `src/api.ts` 调用 Tauri 命令；`commands.rs` 通过统一 `spawn_blocking` 边界转交业务模块，数据库驱动和原生工具不占用窗口线程或异步运行时的工作线程。`state.rs` 管理连接、会话和运行时隧道，`drivers/` 实现各数据库协议。原有命令名称保持兼容。

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

`DbSession::execute_transaction` / `execute_checked_transaction` 是批量 SQL 修改的事务边界。网格更新和删除附带读取时的原始行，生成主键与原值条件，并在提交前校验实际影响行数；冲突回滚整批。MySQL 使用 matched rows 语义，SQL Server 使用驱动的真实执行计数，Oracle 在同一事务块中校验 `SQL%ROWCOUNT`。调用前完整编译和校验，默认实现保持同一会话；SQL Server 使用其事务语法，Oracle 覆写为一次 SQL*Plus 原子脚本，不能提供持久事务的 Snowflake 路径明确拒绝。

同步只用真实完整主键，复合键编码保持值边界和类型。全部读取及检查通过后才开始写入。每批独立事务，不保证整个多批操作原子性；操作期间应停止源和目标的其他写入。

## React 工作台

`src/components/StudioPanel.tsx` 只按工具类型分派，实现在 `src/components/studio/`：查询设计、建模、同步、文件操作、分析、任务、AI、项目包、目录和引擎工具各自维护。`shared.tsx` 仅包含共有连接字段、结果网格、操作锁及确认交互。

请求契约在 `src/api.ts` 具名声明。`FileExportResult.kind/note` 区分普通导出、原生备份、SQLite 表脚本和纯数据脚本；界面按真实结果展示完成、警告或失败。

## 文件与恢复

`data/delimited.rs` 统一网格与工作台 CSV/TSV 读写。空单元格保留空字符串，完整 `\N` 为 NULL；只对“一个或多个反斜杠后接 N”的保留值族加一层转义，普通 UNC 路径不变。文本不猜测数值类型，数值列使用经过语法校验的精确十进制字面量，避免 f64 中转。字段数和重复表头在写入前检查。

内置备份写临时文件成功后替换，恢复按语句流式预检再执行；总文件不再受 20MB 限制，单条 SQL 保留 20MB 上限。外部 PostgreSQL 脚本/归档分别使用受限 psql/pg_restore，MySQL 非交互恢复使用 binary-mode。不能等价传递用户连接安全选项时拒绝原生操作。

本地配置读取失败返回错误，更新使用跨进程文件锁、唯一临时文件和上一版备份。任务执行结果与配置 revision 合并分离，避免暂停、编辑任务后隐瞒运行结果。密钥来源冲突仅根据已有认证密文选择唯一可用候选，不覆盖任何密钥。

## 编辑状态

查询结果固定绑定执行连接、数据库和 SQL；请求编号阻止旧响应回填。格式化、参数替换和执行计划同样校验上下文。表编辑由即时操作锁防重入，文档库部分提交错误携带已完成条数，前端冻结写入直至刷新核对。

MongoDB 的 `originalRows` 保留每行实际存在的字段和 canonical Extended JSON 类型，与展示中的缺失值分开。Redis 使用单键 Lua 比较并写入；两者仍不提供跨行整批事务。查询草稿与历史是本机 SQL 原文，损坏恢复记录不会被自动覆盖。Studio 工具按领域懒加载，隐藏标签保留组件状态。
