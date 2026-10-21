# 模块边界

React 界面经 `src/api.ts` 调用 Tauri 命令；`commands.rs` 通过统一 `spawn_blocking` 边界转交业务模块，数据库驱动和原生工具不占用窗口线程或异步运行时的工作线程。`state.rs` 管理连接、会话和运行时隧道，`drivers/` 实现各数据库协议。原有命令名称保持兼容。

## Rust 工作台

`src-tauri/src/studio.rs` 只声明领域模块并导出命令使用的接口。领域模块显式导入依赖，内部辅助项使用 `pub(super)`，不靠通配导入共享全部实现。

| 模块 | 负责内容 |
| --- | --- |
| `studio/types.rs` | 工作台请求、响应和持久数据契约 |
| `studio/storage.rs` | 工作台本地文件及通用值转换 |
| `studio/table_data.rs` | 受限完整读取入口、生成与脱敏 |
| `studio/read_batches.rs` | 导出与内置备份共用的独立只读连接、快照和主键续读 |
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

同步只用真实完整主键，复合键编码保持值边界和类型。支持的 SQL 引擎采用独立快照、keyset 分页、磁盘索引及磁盘差异计划；全部读取及检查通过后才开始写入。固定独立目标会话避免重连后写错目标和影响编辑器事务。每批独立事务，原始行条件继续检测目标变化，不保证整个多批操作原子性。

## React 工作台

`src/components/StudioPanel.tsx` 只按工具类型分派，实现在 `src/components/studio/`：查询设计、建模、同步、文件操作、分析、任务、AI、项目包、目录和引擎工具各自维护。`shared.tsx` 仅包含共有连接字段、结果网格、操作锁及确认交互。

请求契约在 `src/api.ts` 具名声明。`FileExportResult.kind/note` 区分普通导出、原生备份、SQLite 表脚本和纯数据脚本；界面按真实结果展示完成、警告或失败。

## 文件与恢复

`data/delimited.rs` 统一网格与工作台 CSV/TSV 读写。空单元格保留空字符串，完整 `\N` 为 NULL；只对“一个或多个反斜杠后接 N”的保留值族加一层转义，普通 UNC 路径不变。文本不猜测数值类型，数值列使用经过语法校验的精确十进制字面量，避免 f64 中转。字段数和重复表头在写入前检查。

内置备份写临时文件成功后替换，恢复按语句流式预检再执行；总文件不再受 20MB 限制，单条 SQL 保留 20MB 上限。外部 PostgreSQL 脚本/归档分别使用受限 psql/pg_restore，MySQL 非交互恢复使用 binary-mode。不能等价传递用户连接安全选项时拒绝原生操作。

`read_batches.rs` 为 CSV/TXT/JSON/XML 表导出和内置备份复用一次元数据及 COUNT，保持显式投影的物理列顺序。SQLite/PostgreSQL/MySQL/MariaDB 的独立只读快照中，安全非空主键采用至多 500 行的 keyset 查询，不累积 OFFSET。MySQL/MariaDB 仅 InnoDB 实体表可走此路径。SQLite 游标按实际值而非声明类型生成；同一 COUNT 预检 NULL、BLOB、大整数、非有限实数及 NUL 文本，无法无歧义续读时在首批前回退。文档库以单次受限结果中实际发现的列作为表头，不能复用首条样本的列下标。

不支持安全续读的表一次最多读取 20 万行；普通导出返回截断说明，备份发现不完整时保留旧产物并报错。XLSX/DBF/Parquet/PDF 继续受限组装；流式格式边读边写临时文件，成功结束快照、关闭文件并通过取消检查后才发布。不以应用批次推断操作系统 RSS，驱动内部缓冲及宽行仍影响峰值。

本地配置读取失败返回错误，更新使用跨进程文件锁、唯一临时文件和上一版备份。任务执行结果与配置 revision 合并分离，避免暂停、编辑任务后隐瞒运行结果。密钥来源冲突仅根据已有认证密文选择唯一可用候选，不覆盖任何密钥。

## 编辑状态

网格和 CSV/文件导入的自动事务使用独立会话，不提交或回滚查询编辑器的事务。独立连接固定沿用打开时的连接与隧道配置；SQLite 已有写事务可能阻止第二个写入者，此时明确报告锁冲突并保留原事务。SQL Server 执行计划也使用独立会话，成功或失败均尝试关闭 SHOWPLAN。

查询结果固定绑定执行连接、数据库和 SQL；请求编号阻止旧响应回填。格式化、参数替换和执行计划同样校验上下文。表编辑由即时操作锁防重入，文档库部分提交错误携带已完成条数，前端冻结写入直至刷新核对。

MongoDB 的 `originalRows` 保留每行实际存在的字段和 canonical Extended JSON 类型，与展示中的缺失值分开。Redis 使用单键 Lua 比较并写入；两者仍不提供跨行整批事务。查询草稿与历史是本机 SQL 原文，损坏恢复记录不会被自动覆盖。Studio 工具按领域懒加载，隐藏标签保留组件状态。

BSON/JSON 编辑区保留结构化值，未修改即失焦不会产生变更；提交前先完成活动单元格校验，任何无效草稿都会阻止整批写入。查询编辑器及 Monaco 按需加载；目录按连接、数据库、模式共享，先提供表名再分批补列。连接更新、断开及刷新使缓存失效，迟到目录不能回写。草稿在连续输入时合并保存，并在切换、隐藏和关闭前补写。

BI 预览、保存和自动刷新统一调用后端单条只读查询入口。SQLite/PostgreSQL/MySQL/MariaDB 使用独立只读快照；其余 SQL 引擎执行语法只读检查，仍不能保证 SELECT 函数无副作用。同一面板不重叠执行刷新，连接、数据库、SQL 等上下文变化会取消任务并丢弃旧结果。

SSH 在一个非阻塞循环内服务多个通道，保留短写数据并重试通道 CLOSE；HTTP CONNECT 使用可回收的连接工作线程。两者支持半关闭和背压；HTTP 响应头完整消费到边界，保留同包中的数据库问候数据。代理地址在建立隧道时解析，工作线程停止不等待 DNS。系统计划每分钟唤醒到期检查，由保存的各任务决定间隔。

## 前台任务生命周期

`operations.rs` 管理 queued/running/succeeded/failed/cancelled 状态、取消令牌、阶段计数和已知提交量。命令先 claim 一次任务 ID，再进入后台执行；前端 `src/operations.ts` 统一创建、轮询、取消、终态确认和释放。IPC 失败本身不证明后台停止。queued 取消可直接终结；running 取消仅提出请求，由执行器在安全点终结。

中断回调绑定到持有的驱动会话租约。发送信号与租约退役共用锁，退役在解锁会话之前完成；取消信号在同一租约内短暂重试，防止首次信号早于 SQL 真正开始。任意新操作不能复用旧取消回调。SQLite 使用 interrupt handle，PostgreSQL 使用 cancel token，MySQL 使用同安全连接策略的独立 KILL QUERY。事务 COMMIT 不挂驱动中断回调；应用批量写入在语句边界检查取消并回滚当前批。

`studio/stream_sync.rs` 负责完整扫描、磁盘差异计划、整计划预检和分批写入；`scan_stage.rs` 负责最多 500 行的 keyset 读取、4MiB 页缓存的 SQLite 暂存及 RAII 清理；`sync_key_semantics.rs` 拒绝不能精确表达的文本主键等价规则。源、目标分别建立快照，全部数据和计划落盘，不把总行数当作内存限制。当前不支持崩溃后断点恢复，也不会在启动时无条件删除其他进程可能正在使用的暂存目录。

文件导出通过 `AtomicOutput` 发布，取消或失败保留旧文件。原生子进程用可轮询生命周期和私密限量错误日志，取消 kill 后 wait 回收；还原使用独立连接并保守统计确认提交的写语句，不能把“已执行语句”当作“已提交数据行”。
