# DataHarbor 功能对照

DataHarbor 是独立产品。这份清单用来记录已经做到的能力和还没做的能力。对照范围来自公开的数据库客户端功能，不使用那些产品的商标、图形或专有资源，也不表示隶属或授权。

状态：

- **已实现**：当前版本可以真正执行。
- **部分**：能用，但有明确限制。
- **未做**：没有可用实现。

当前为 0.1 早期预览。“已实现”说明有实际代码路径，不能替代对应数据库版本和系统的端到端验收。自动化覆盖与未验证项见 [TESTING.md](TESTING.md)。

## 引擎

| 对象 | 状态 | 说明 |
| --- | --- | --- |
| MySQL / MariaDB | 已实现 | `mysql` crate，rustls。SSL 与信任自签证书。 |
| PostgreSQL | 已实现 | `postgres` + native-tls。按数据库分开会话。 |
| SQLite | 已实现 | `rusqlite`。含示例书店。 |
| MongoDB | 已实现 | 官方驱动。`find`、`aggregate`、JSON 命令。文档网格按 `_id` 更新。 |
| Redis / Garnet | 已实现 | 单机、Cluster、Sentinel。Garnet 走同一套 Redis RESP，连接预设端口 6379。键浏览和常用类型的读写。查询编辑器执行 Redis 命令。服务器不认识的命令会返回错误。 |
| SQL Server | 已实现 | `tiberius`，启用 native-tls 后端。默认不加密；强制加密的服务器要打开 SSL。服务器端验收另行记录。 |
| Oracle | 部分 | 调用本机 `sqlplus`，不链接 OCI。没有不依赖 OCI 的纯 Rust 线协议；`oracle` / `sibyl` 都要 Instant Client，因此不捆绑。没有 `sqlplus` 时会给出明确错误。 |
| Snowflake | 部分 | SQL API。密码栏保存 PAT 或 OAuth 令牌。仓库和模式是连接字段。 |
| 达梦 | 部分 | `dameng` 0.1 的纯 Rust DM8 线协议，默认端口 5236。不是 ODBC，也不捆绑厂商 DPI。目录查询用 `ALL_USERS` / `ALL_TABLES` / `V$VERSION`。TLS 使用系统信任库，不能跳过证书校验。 |
| 云厂商与兼容库 | 部分 | AWS、Azure、GCP、阿里云、腾讯云、华为云、OceanBase MySQL 模式、TiDB、PolarDB、openGauss、GaussDB、KingbaseES、IvorySQL 是预设，走 MySQL 或 PostgreSQL 协议。OceanBase Oracle 模式仍没有单独驱动。 |

## 连接与安全

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| 主机、端口、用户、密码、库、SQLite 文件 | 已实现 | |
| SSL / TLS、信任自签证书 | 已实现 | 没有客户端证书和证书固定。 |
| SSH 隧道 | 部分 | libssh2 本地转发，密码或私钥。认证前严格核对用户 known_hosts 的主机和端口；未知、变化或不可读拒绝。不会自动关闭 TLS 校验。PG/SQL Server 保留远端证书身份；MySQL/MariaDB、MongoDB、达梦不支持严格 TLS 隧道组合。 |
| HTTP 隧道 | 部分 | HTTP CONNECT，无代理账号。 |
| Redis Cluster + SSH | 部分 | 为列出的每个 `host:port` 各建一条隧道，并写入地址映射。Cluster 在有映射时自己读 `CLUSTER SLOTS`，把广播地址改写成隧道或手工映射，并跟一次 `MOVED` / `ASK`。没有映射的地址仍按广播地址直连。`redis` 0.27 没有公开的地址改写回调，所以这条路径不使用 `ClusterClient`。 |
| PAM / LDAP / Kerberos / Windows 集成认证 | 部分 | MySQL `cleartext` / LDAP / PAM 会打开 cleartext 插件，服务器上仍要有对应插件。PostgreSQL GSSAPI 在连接前拒绝，原因见文末。SQL Server Windows 身份验证在 Windows 构建里使用 `tiberius` 的 `AuthMethod::Integrated`（特性 `winauth`，仅 `cfg(windows)` 编译 SSPI）。macOS 与 Linux 能编过，但会明确拒绝。 |
| 连接 URI | 已实现 | 在新建连接里粘贴 `mysql://` 等地址即可填表。 |
| 分组、颜色、加密存储 | 已实现 | 分组用 `/` 嵌套，例如 `云厂商/阿里云`。 |

## 查询与对象

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| 高亮、运行、分页、错误停止 | 已实现 | |
| 美化 / 压缩 | 已实现 | 本地排版，不调用外部 SQL 格式化服务。 |
| 参数查询 | 已实现 | `:name` 与 `@name`，不替换字符串里的同名文本。 |
| 执行计划 | 部分 | MySQL / PostgreSQL / SQLite / SQL Server 解析成可着色的树。Oracle 和 Snowflake 多为文本节点。不是独立的优化器工作台。 |
| 补全、悬停、跳转 | 部分 | 按已加载的表和列补全，检查引号、括号和未知表名，悬停列类型，表名跳到内存中的结构声明。没有语义重构。 |
| 可视化查询构建器 | 已实现 | 拖入或点击表，配置 JOIN 与 WHERE，生成 SQL 并打开编辑器。 |
| 代码片段 | 已实现 | 保存在本机 `snippets.json`。 |
| 对象设计器 | 已实现 | 表、视图、索引、过程的 SQL 预览和执行。不是 PL 调试器。 |
| 命令控制台 | 部分 | 查询编辑器即控制台。Redis / MongoDB 使用各自语法。 |
| 存储过程调试 | 部分 | PostgreSQL 在已安装 `pldbgapi` 时用两条连接：控制会话 `pldbg_create_listener`、全局断点、`pldbg_wait_for_target`、步过 / 步入 / 继续，并读取变量和栈；目标会话执行调用 SQL。等待限 20 秒。扩展要由超级用户安装，本程序不自动安装。MySQL 与 Oracle 明确说明没有调试器。 |

## 数据

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| 网格筛选、排序、编辑、提交 | 部分 | 修改先完整预校验，再按驱动提交同会话事务；SQL Server 用 BEGIN TRANSACTION，Oracle 为单次 SQL*Plus 事务块（生成语句单条上限 8KB，待实机验收）。Snowflake SQL API 无持久会话事务，拒绝网格/CSV 原子批量修改。网格提交检查原始快照和实际影响行数，SQL 冲突整批回滚。文档库逐条提交，部分失败阻止网格直接整批重放。 |
| 表单视图、JSON 查看 | 已实现 | |
| CSV / TXT / JSON / XML / XLSX / DBF / PDF | 已实现 | 单次最多 20 万行，这是内存上限，不再是 2000 行产品限制。PDF 嵌入本机中文字体。 |
| Parquet | 已实现 | `parquet` 59 + Arrow。导入导出把单元格写成可空 UTF-8，默认 Snappy。读写 Snappy、gzip、zstd、lz4。lz4 用纯 Rust 的 `lz4_flex`。zstd 用官方 crate 自带的 C 源码，靠 cc 编译，和 Tauri 在 Windows、macOS 上需要的 C 工具链相同。没有 brotli。 |
| 外键取值 | 已实现 | 网格和表单里，有外键的列可以打开引用表，搜索任意文本列，点一行写回被引用列。一次最多 40 行。跨模式外键按当前模式查找。 |
| 二进制 / 图片 / HTML | 已实现 | 二进制列可看可改十六进制，超过 1MB 只读前 4096 字节。BLOB / bytea 可预览 PNG、JPEG、GIF、WebP、BMP，并用本地文件替换（8MB 内）。文本列可预览 HTML（不执行脚本）或 http(s) 网址。写回数据库时二进制用各引擎的十六进制字面量。 |
| Access / ODBC | 部分 | 有 `mdb-export` 时读 Access；有 `isql` 时按 DSN 导入。没有这些工具就报清楚，不内嵌驱动。 |
| 结构同步 | 已实现 | 对比列、索引、外键、视图。可生成删除和修改。跨引擎保留可证明的精度、长度和整数范围，无法保证无损时拒绝自动转换；修改已有类型也需危险操作确认。语句可勾选。SQLite 的类型变更和外键以警告注释给出。 |
| 数据传输 / 同步 | 部分 | SQLite/MySQL/MariaDB/PostgreSQL 有主键表采用独立快照、keyset 分页和临时磁盘索引，全量无 20 万行上限，应用批次最多 500 行；其他引擎及无主键传输保留受限完整读取。读取不完整时拒绝开始写入。同步要求源、目标真实完整且兼容的主键，复合键用 JSON 数组编码，拒绝重复键。更新/删除核对目标原值；每批独立提交，失败报告已提交批次和条数，不保证全任务原子回滚；执行时停止其他写入。 |
| 备份 / 还原 | 部分 | 优先 `mysqldump`、`pg_dump`、`mongodump` / `mongorestore`。SQL Server 提交 `BACKUP` / `RESTORE`，路径必须在服务器上。内置遍历全部表，任一表读取不完整即拒绝成功；SQLite 回退为表结构和数据脚本（不含独立索引、视图、触发器），其他 SQL 引擎仅为 INSERT 数据脚本，无完整对象或一致性快照保证。外部 PostgreSQL 普通脚本使用支持 restricted 模式的 psql，自定义格式使用 pg_restore；内置脚本流式预检及还原，单语句限 20MB，总文件可超过 20MB。不能等价继承连接安全策略时拒绝原生工具。 |
| 数据生成、脱敏、字典、结构分析 | 已实现 | 生成最多 500 行。脱敏是整列替换。 |
| Mongo 导入导出 | 部分 | 集合可走 JSON 导入导出。没有单独的 mongoimport 兼容全部选项。 |
| Redis Pub/Sub | 部分 | 发布走已有连接。订阅另开直连，SSH 下要求端口可达。 |

## 建模、图表、自动化

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| ER 图 | 已实现 | 可拖动，位置记在本机，可缩放。导出 SVG、PNG，以及带中文字体的 PDF 结构清单。 |
| 概念 / 逻辑 / 物理模型 | 部分 | 可保存三层，反向读取物理模型，正向输出 CREATE TABLE。 |
| 导出 SQL / HTML / PDF | 部分 | HTML 仍可打印。PDF 使用本机中文字体（Droid、Noto、微软雅黑或宋体）。没有字体时会说明，并保留 HTML。 |
| BI | 已实现 | 柱、折线、饼、面积、散点。多页签。打开看板时可按秒刷新。数据来自真实查询。 |
| 定时任务 | 部分 | 托盘隐藏后仍每 15 秒检查。`--run-jobs` 给定时器调用，冷启动主动建连，失败返回非零；跨进程文件锁防止并行重复执行。可写出 cron / launchd / schtasks 脚本，默认不覆盖现有 crontab。一个任务可带多个连接。SMTP 由用户提供，密码加密保存，可带附件。 |
| 用户与监控 | 部分 | 能执行各引擎的账号和会话查询。权限不足时返回说明，不会伪造列表。 |
| AI | 部分 | 用户自己的接口和密钥。支持解释、优化、格式化、方言转换、修复、对话、多个房间、分叉、两个模型对照，以及把补全写进未提交的网格。没有密钥就拒绝；切换服务 origin 需重新提供密钥，远程 HTTPS、本机可 HTTP，不自动跟随重定向。 |
| 项目包 | 已实现 | 导出连接密文、片段、任务、看板、模型、收藏。地址、账号、路径、SQL 仍是敏感资料。导入新任务默认禁用；已有同 ID 本地任务保持原样。无法用本机密钥解密的导入凭据不会写入本机配置，界面提示重新填写；原项目包保持原样。没有云端账号服务。 |

## 仍未实现

- Redis 与 Oracle 的显式 SSL 配置；Snowflake 的本机隧道重定向。不支持的选项会明确拒绝。
- Oracle 12c 以前版本的自动分页；当前网格与查询使用 OFFSET/FETCH，不插入额外辅助列。
- MySQL/MariaDB、MongoDB、达梦的严格证书身份校验与本机隧道组合，当前驱动无法单独传入证书主机名。

- Oracle OCI 直连，以及不依赖 `sqlplus` 的线协议。`oracle` / `sibyl` 都要 Instant Client，本程序不捆绑。
- PostgreSQL GSSAPI / Kerberos。`rust-postgres` 0.19 依赖 `tokio-postgres` 0.7。协议层能解析 `AuthenticationGss`（认证码 7）和 `AuthenticationGssContinue`，但 `connect_raw` 对这两个分支直接返回 “unsupported authentication method”，没有 GSS token 交换，也不调用 `gss_init_sec_context`。`tiberius` 的 `integrated-auth-gssapi` 只用于 SQL Server 的 TDS 登录，不能拿来做 PostgreSQL 启动包。链接 `libpq` 会换成另一套客户端，不在当前依赖里。因此连接在握手前拒绝，而不是假装协商成功。
- SQL Server Windows 身份验证在 macOS 和 Linux 上。`winauth` 特性已打开，但 `tiberius` 把 SSPI crate 和 `AuthMethod::Integrated` 限制在 `cfg(windows)`。非 Windows 构建会编译，并在选择 Windows 认证时返回明确错误。
- MySQL 存储过程调试和 Oracle `DBMS_DEBUG`。没有公开的单步协议，Oracle 也没有 OCI。
- 未出现在隧道列表或地址映射里的 Redis Cluster 广播地址。`redis` 0.27 的 `ClusterClientBuilder` 没有改写槽位地址的回调，未映射的地址仍会直连。
- Parquet 的 brotli 压缩。
- 达梦跳过证书校验。`dameng` 0.1 的 TLS 使用默认 `TlsConnector`，没有接受无效证书的开关。
- 超过 1MB 的二进制单元格不能在十六进制窗口里整段改写。
- HTML 预览不执行单元格里的脚本。
- OceanBase Oracle 模式。Garnet 没有独立协议，已按 Redis RESP 连接。

## 后续优化顺序

当前重点验收 SQLite、MySQL、PostgreSQL 的编辑、CSV 往返及事务行为，并补 MongoDB/Redis 的独立容器回归。驱动代码存在不代表各版本、集群、认证组合或平台均已验收。

后续工作按以下顺序安排：

1. 各原生工具、其他数据库和桌面平台的真实安装及备份恢复验收。
2. 长任务取消、进度和 SQL 主键流式扫描已实现；后续扩展其他引擎快照与比较语义、跨重启断点恢复，保持现有完整读取保护。
3. 进一步拆分编辑器依赖、测量实际启动/交互性能；完善键盘及屏幕阅读器验收。
4. 正式发布的签名、公证、安装升级验证与第三方授权人工核对。当前手动产物继续标为预览，不能把缺少材料的授权项当成已审计。

## 本轮 P2 已完成

- 前台查询、同步、导入导出和备份恢复统一任务进度与取消；关闭运行标签先取消并等待实际停止。
- SQLite/PG/MySQL 驱动中断，其他驱动在安全边界停止；后台进程会被回收，取消不冒充回滚。
- 有主键 SQL 表的完整磁盘暂存/差异计划，超过 20 万行实测；源/目标一致读分别建立，固定独立写连接。
- 所有正常结束路径清理临时索引；文本主键的 NOCASE、CI/AI、PAD SPACE、自定义类型等无法确认的组合在零写入前拒绝。
- MongoDB 实库测试已在本轮补齐，实际版本和验证结果见 TESTING.md。
