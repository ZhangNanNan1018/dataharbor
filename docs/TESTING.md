# 验证方法与覆盖边界

## 本地回归

```sh
npm ci
npm test
npm run build
cd src-tauri
cargo test --lib --locked
```

测试使用临时配置目录和数据库，强制文件测试密钥，不读取个人系统凭据或已保存连接。覆盖 SQLite 的查询、分页、改行、事务回滚、CSV、同步主键、读取截断、内置脚本和项目导入；SSH/TLS 的配置与信任校验使用本地夹具。

## 专用服务器往返测试

根目录 `compose.test.yml` 创建独立数据库，默认仅在本机端口 13306、15432、27027 和 16379 监听，账号密码仅用于测试。请勿把测试端口指向业务数据库。

PowerShell：

```powershell
docker compose -p dataharbor-tests -f compose.test.yml up -d --wait
$env:DATAHARBOR_TEST_MYSQL_PORT = '13306'
$env:DATAHARBOR_TEST_POSTGRES_PORT = '15432'
$env:DATAHARBOR_TEST_PG_CONTAINER = docker compose -p dataharbor-tests -f compose.test.yml ps -q postgres
$env:DATAHARBOR_TEST_MONGO_PORT = '27027'
$env:DATAHARBOR_TEST_REDIS_PORT = '16379'
Set-Location src-tauri
cargo test --lib --locked -- --ignored --test-threads=1
Set-Location ..
docker compose -p dataharbor-tests -f compose.test.yml down -v
```

专用测试覆盖 MySQL 8.4、PostgreSQL 16、MongoDB 7 和 Redis 7。SQL 测试验证复合主键、实际修改行数、并发冲突、整批回滚、高精度 DECIMAL、大整数、微秒时间、二进制及 CSV 两入口往返。MongoDB 验证过滤计数、稳定分页、稀疏且保留 BSON 类型的原始快照、并发冲突及部分提交错误。Redis 验证原子比较更新、类型变化、TTL 保留、列表删除和跨 500 行分页。测试表、数据库或键名随机，结束只清理自身数据。

GitHub CI 在三个桌面系统执行构建与回归，在独立 Linux 服务容器执行上述服务器测试。工作流配置存在不代表已经在 GitHub 运行通过。

2026-10-20 本机验证：Windows、Node.js 24.15.0、Rust 1.94.0，前端构建通过；默认 Rust 回归 71 项通过，另外两项专用服务器测试在 MySQL 8.4、PostgreSQL 16 容器中通过。服务器测试还用真实 PostgreSQL sequence 验证带函数的查询不会为自动计数被额外执行。后续修改应重新核对该记录。

## 发布验收记录

维护者需要逐个平台和引擎记录版本、构建、启动与主流程结果。当前自动化不能证明下列情况已经验证：

- macOS/Linux 的实际安装和桌面窗口交互；
- SQL Server、Oracle、达梦、Snowflake 的服务器端端到端验收，以及 MongoDB/Redis 的更多版本和集群组合；
- 真实跳板机、证书链、代理、集群故障切换；
- 多用户并发写入时的一致性快照或全任务原子回滚。

后台任务使用配置文件锁和跨进程执行锁；桌面与外部 `--run-jobs` 共用同一配置目录时不会并行运行同批任务。任务冷启动会主动建连，任何实际执行失败都会令 CLI 返回非零；执行中暂停/编辑仅阻止旧结果覆盖配置，不会隐瞒已执行操作的失败。进程意外退出时锁由操作系统释放，但任务不承诺数据库写入的 exactly-once 语义。

修改相关模块后运行对应回归，再执行整体构建。不得把未运行或仅静态检查的项记录成通过。

## 本轮新增回归范围（2026-10-21）

- 前端使用 Vitest、Testing Library 和 jsdom，真实挂载 AntD 组件；Tauri IPC、Monaco 和专用单元格编辑器使用测试替身。覆盖活动标签保存、防重复提交、部分提交后禁止重放、失败刷新、换库及迟到响应、稀疏原始快照、多结果导航、草稿损坏保留和恢复、工作台懒加载状态、AI 服务切换。
- 凭据测试注入模拟钥匙串，不读取或修改个人系统凭据。验证暂时不可用、已存密文、冲突密钥及唯一可解候选。
- SQLite 回归覆盖整批冲突回滚、CSV/TXT 引号和换行、NULL/空字符串/UNC 路径、超过 20MB 的内置备份恢复、坏尾部预检不写入、分批失败进度、任务冷启动和跨进程互斥。
- SMTP 使用本机临时监听验证域名与分段协议；不会向真实邮箱发送邮件。
- 原生工具参数、临时凭据清理、路径发现和 psql restricted 模式准备由离线夹具验证。PostgreSQL 原生 COPY 脚本另通过容器内 pg_dump/受限 psql 和实际数据回读验证；其他原生工具备份→还原、真实证书/跳板机以及 SQL Server/Oracle/达梦/Snowflake 仍需对应环境验收。

前端交互测试不能替代真实 WebView、屏幕阅读器或桌面响应时间验收。CI 工作流已接入前端回归和四类数据库测试；本地通过也不能替代远程 CI 的实际结果。

## 第一轮 P1 本机验收结果（2026-10-21，历史记录）

- Windows 前端交互回归：22 项通过；生产构建通过。主包按工作台领域懒加载后约 4.50MB，仍有大包警告，未将其视为实际启动性能验收。
- Rust 常规回归：114 项通过；7 项外部数据库测试默认 ignored。
- 显式实库回归：6 项通过，覆盖 MySQL 8.4、PostgreSQL 16、Redis 7；包含 PostgreSQL 原生 pg_dump COPY 脚本经受限 psql 恢复后的数据核对。
- MongoDB 的 1 项实库回归未运行：Docker Hub 及备用镜像源的镜像下载持续停滞，已停止下载；相关 helper 回归通过，但不能替代真实服务器验收。CI 已配置 MongoDB 服务，仍需在镜像可用的环境执行。
- Windows 调试二进制构建通过；库目标改名为 dataharbor_lib，消除了与可执行文件调试符号重名的构建警告。
- Git 差异检查、测试 Compose 配置校验通过；第三方授权重新生成，仍有 50 项需要人工核对。未执行远程 GitHub CI、正式签名发布、macOS/Linux 桌面或其他数据库实机验收。
- 验证使用本轮独立的临时数据库和配置；测试结束已清理本轮容器，不操作业务数据库。

## P2 任务与流式同步回归（2026-10-21）

常规测试新增：任务 ID 单次认领、queued 取消、租约退役与中断重试、取消后已知提交量、SQL 结果失败终态、显式事务不误计提交、导入取消回滚本批、输出取消保留旧文件、原生子进程终止回收、还原取消与独立事务。

流式同步测试使用 200003 行完整同步，并断言应用批次不超过 500 行；覆盖读取/对比/预检阶段取消零写入、500 行提交后取消、末批坏值全计划预检、复合主键与列顺序、快照期间并发写入、修改连接后固定原目标、编辑器原有事务保持、临时目录清理及不安全文本主键拒绝。内存验证是算法批次边界，不是操作系统 RSS 压测；宽行与驱动内部缓冲仍可能提高峰值。

显式实库测试新增 MySQL/PostgreSQL 查询中断、取消后不执行后续 INSERT、快照一致性，以及流式扫描→磁盘计划→实际增删改完整往返。MongoDB 使用 7.0.45，已补齐稀疏原始快照、BSON 类型、并发冲突及部分提交回归。只运行下列测试时仍需按上文启动独立容器并设置端口：

```sh
cargo test --lib --locked drivers::control_tests:: -- --ignored --test-threads=1
cargo test --lib --locked studio::sync::streaming:: -- --ignored --test-threads=1
cargo test --lib --locked drivers::document_tests:: -- --ignored --test-threads=1
```

前端新增任务创建期间取消、轮询乱序、取消失败重试、传输失败但后台仍运行、终态释放、关闭运行标签等待停止、同步部分提交后取消等真实组件回归。模拟 IPC 与真实驱动测试分开记录，不把组件测试当作 WebView 实机验收。

## 本轮 P2 最终验证结果

- 前端：4 个测试文件、31 项全部通过；类型检查与生产构建通过。覆盖取消请求、终态确认、任务标签关闭、迟到状态、部分提交以及已知空任务 0/0 展示。
- Rust：使用 `--include-ignored --test-threads=1` 执行 157 项，全部通过，无跳过；其中 14 项是显式实库回归。
- 实库：MySQL 8.4、PostgreSQL 16、MongoDB 7.0.45、Redis 7 均通过对应回归。MongoDB 官方镜像已补齐；临时实例曾退出，重启该测试实例后通过，不能据此推断业务部署稳定性。
- 全量同步：200003 行完整回读核对，应用批次最多 500 行；支持的 SQL 流式路径已移除旧 20 万行上限。读取/对比/校验取消零写入，提交后取消保留准确已提交计数。
- 查询与文件：SQLite/PG/MySQL 的实际中断、取消后不继续执行 INSERT、事务回滚、外部进程回收、还原独立连接、输出原件保留均有回归。
- 已做独立源码复核并修复 SQL 失败终态、未提交数量误报、取消信号早于 SQL 启动的竞态和 queued 记录回收问题。
- Windows 调试二进制构建通过；出现本机增量缓存目录权限警告，但编译和链接完成。Git 差异检查、测试 Compose 配置校验通过；本轮四个临时测试容器及其测试卷已清理，未操作业务数据库。

仍未包含真实 WebView 完整交互、macOS/Linux 安装、跨引擎所有版本/认证组合、操作系统 RSS 基准或崩溃断点恢复。源/目标快照分别建立；文本主键无法确认精确等价规则时会在零写入前拒绝。跨批次取消不撤销已提交数据；原生还原副作用仍需核对。

## 表导出与内置备份性能回归

定向运行新读取模块的回归：

```sh
cargo test --lib --locked studio::read_batches::tests:: -- --test-threads=1
```

测试使用临时 SQLite 文件，覆盖 200003 行 CSV 导出与内置备份、CSV/TXT/JSON/XML 的格式和转义、复合主键及物理列顺序、跨页和跨表快照、NOCASE 列与 BINARY 主键排序、动态类型主键、文档库采样列与结果列不同、取消及末批坏值保留旧文件。SQLite 含 BLOB、大整数、非有限实数或 NUL 文本的主键会在首批之前回退；无主键、可空主键或无快照的路径断言只做一次受限读取。

读取计数夹具对 200003 行主键表断言：列结构 1 次、COUNT 1 次、数据查询 401 次、每批最多 500 行、无 OFFSET；另用 EXPLAIN QUERY PLAN 检查整数主键续读仍走索引。它验证查询次数、读取完整性及应用批次边界，不代表实际启动时间、操作系统 RSS 或所有数据库的性能基准。驱动内部缓冲与单行大小不在这项边界内；实际运行结果应由本轮验收记录另行填写。

## 可靠性与性能优化验收（2026-10-21）

- 前端整合回归 7 个文件、63 项通过；最后补充的 Snowflake fixed 精确数值与编辑器模型复用相关定向回归通过。类型检查及最终生产构建通过。主入口从 4526.00 kB 降至 1153.67 kB（gzip 366.34 kB），查询编辑器按需块 3339.26 kB。这是资源加载尺寸改善，未测量真实启动耗时。
- 隔离 Chromium 验证真实 Monaco 渲染、SQL 高亮、Ctrl+F 查找、F1 命令及关键词补全。没有新增编辑器控制台错误；仍有 AntD 既有弃用提示。这不能代替原生 WebView 或 macOS/Linux 桌面验收。
- Rust 完整回归使用 `cargo test --lib --locked -- --include-ignored --test-threads=1`，190 项全部通过，无跳过；包括 16 项真实数据库测试和 1 项本机 SSH 协议测试。最后完善通道关闭和代理解析后，隧道 9 项、计划调度 3 项再验证通过，含新增 2 项测试。
- MySQL 8.4、PostgreSQL 16 实际验证编辑器持有未提交 UPDATE/INSERT 时，独立网格和 CSV 成功或失败均不结束原事务；BI 看不到未提交数据，也不改变编辑器事务。MongoDB 7、Redis 7 的既有真实读写、冲突和分页回归通过。
- 200003 行 CSV 导出与内置备份回读、单次结构/COUNT、401 批与 500 行上限、快照、动态类型、取消和原件保留回归通过。取消中断返回取消状态；无键和歧义游标不会勉强分页。没有进行操作系统 RSS 基准。
- HTTP 本机代理验证双连接、碎片响应头、问候数据、半关闭及取消回收。SSH 夹具保持空闲通道，另外三通道并发传输并反复创建关闭 50 条短连接；信任文件仅位于临时目录，不读取个人 SSH 密钥。
- 本轮四个数据库容器及其临时数据已清理，未使用业务数据库。未执行远程 CI 或发布；SQL Server SHOWPLAN、Snowflake 结果及异步处理使用离线夹具，仍需真实服务验收。
- Windows 最终调试二进制 `cargo build --bin dataharbor --locked` 构建成功。构建期间曾出现本机增量缓存目录权限警告，最终重新构建完成；第三方授权、正式签名安装包和跨平台桌面验收不在本轮结果内。

SSH 协议测试使用可选的 Python 测试依赖，未加入生产依赖；数据库集成 CI 安装它后运行全部 ignored 测试，工作流仍仅手动触发。只验证本机 SSH 时：

```sh
python -m pip install paramiko==5.0.0
cd src-tauri
cargo test --lib --locked tunnel:: -- --include-ignored --test-threads=1
```
