# DataHarbor

DataHarbor（包名 `dataharbor`）是一个独立的跨平台桌面数据库客户端，支持 Windows 与 macOS（开发机上也可在 Linux 运行）。它能管理连接、浏览对象、写 SQL、改数据，并做导入导出、同步、备份、建模和看板。

功能范围、当前限制和后续规划见 [docs/FEATURE_ROADMAP.md](docs/FEATURE_ROADMAP.md)。

许可证：MIT（见 [LICENSE](LICENSE)）。

当前为 **0.1 早期预览**。功能有实现不表示所有引擎、版本和平台均已验收。贡献、测试及模块说明见 [CONTRIBUTING.md](CONTRIBUTING.md)、[docs/TESTING.md](docs/TESTING.md)、[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)；安全反馈见 [SECURITY.md](SECURITY.md)，第三方授权见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 这一版能做什么

- 连接 MySQL、MariaDB、PostgreSQL、SQLite、MongoDB、Redis（单机 / Cluster / Sentinel）、SQL Server、Snowflake（SQL API + 令牌）。Oracle 通过本机 `sqlplus` 连接，不链接 OCI。云厂商和兼容库是连接预设，不是新协议。
- 新建、编辑、删除、测试连接。颜色、分组、连接 URI 会保存或填入表单。
- 密码和 SSH 密码用 AES-256-GCM 加密后写入本机配置，不明文落盘，也不写进日志。
- SSH 本地端口转发（认证前严格验证 `~/.ssh/known_hosts`），以及 HTTP CONNECT 隧道（无代理认证）。隧道不会自动关闭数据库 TLS 验证；不支持保留证书身份的驱动组合会明确拒绝。
- 对象树：库 / 模式、表或集合、视图、列或键。
- Monaco 查询：运行、分页、美化、压缩、参数替换、按当前库补全表和列、语法标记、悬停、跳到表声明、图形执行计划。Redis 写命令，MongoDB 写 `find` / `aggregate` / JSON。
- 表数据网格：筛选、排序、编辑、表单视图、JSON 查看。文档库走各自的读写，不用假 SQL。
- 导入导出 CSV、TXT、JSON、XML、XLSX、DBF、PDF、Parquet（Snappy、gzip、zstd、lz4，单元格为文本）。网格可按外键取值，查看十六进制和图片，并预览 HTML。Access 依赖 `mdb-export`，ODBC 依赖本机 `isql`。
- 结构对比可生成增删改列、索引、外键和视图，并勾选后执行；过期选择需要重新对比。数据同步只使用真实完整主键，复合键采用保留边界的 JSON 编码；不完整读取会在写入前拒绝。
- ER 图可以拖动、缩放，并导出 SVG、PNG 和带中文字体的 PDF 结构清单。
- 数据生成、脱敏、数据字典、结构分析、BI 看板、概念/逻辑/物理模型、定时任务、账号与会话、AI（自备密钥）、项目包。
- 浅色 / 深色、专注模式。界面默认简体中文。

平台限制见 [docs/FEATURE_ROADMAP.md](docs/FEATURE_ROADMAP.md)。Oracle 需要 PATH 里有 `sqlplus`。PostgreSQL GSSAPI 会在握手前拒绝：`tokio-postgres` 0.7 解析 `AuthenticationGss` 后不完成 token 交换。SQL Server Windows 身份验证只在 Windows 构建里走 SSPI，macOS 与 Linux 会明确拒绝。达梦使用 DM8 线协议。关闭窗口后进程可留在托盘里跑定时任务；系统计划任务执行 `dataharbor --run-jobs`。PDF 需要本机中文字体。

## 环境要求

- Node.js 20 或更新版本
- Rust 1.94 或更新版本（本机使用 stable，CI 固定验证 1.94.0）
- 系统 WebView：Windows 用 WebView2，macOS 用系统 WebKit，Linux 用 WebKitGTK 4.1

### Windows

1. 安装 [Node.js](https://nodejs.org/) 与 [rustup](https://rustup.rs/)。
2. 安装 Visual Studio 生成工具，并勾选「使用 C++ 的桌面开发」。
3. 安装 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)（Windows 11 通常已经自带）。

### macOS

1. 安装 Xcode Command Line Tools：`xcode-select --install`
2. 安装 Node.js 与 rustup。

### Linux（开发）

Debian / Ubuntu 一类系统可以安装 Tauri 2 的系统依赖：

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf \
  libgtk-3-dev libdbus-1-dev pkg-config
```

## 开发与构建

```bash
npm ci
npm run tauri dev
```

开发服务监听 `http://127.0.0.1:1420`。浏览器打开这个地址只能看界面；连接、查询和改数据要在 Tauri 打开的桌面窗口里进行。

打包安装包：

```bash
npm run tauri build
```

分发前先执行 `cd src-tauri && cargo fetch --locked`，返回根目录运行 `npm run licenses` 更新随安装包附带的第三方授权。依赖源需先缓存，授权生成过程离线执行。GitHub 手动安装包工作流会完成这些步骤并上传预览产物。

产物在 `src-tauri/target/release/bundle/`：

| 系统 | 常见产物 |
| --- | --- |
| Windows | `nsis/*.exe`、`msi/*.msi` |
| macOS | `dmg/*.dmg`、`macos/*.app` |
| Linux | `deb/*.deb`、`appimage/*.AppImage` |

只做前端类型检查和静态构建：

```bash
npm test
npm run build
```

后端测试（包含一条 SQLite 浏览、筛选、改行、CSV 往返）：

```bash
cd src-tauri && cargo test --lib --locked
```

## 试用 SQLite

启动桌面窗口后，点工具栏的「示例 SQLite」或欢迎页的「打开示例书店」。程序会在配置目录创建 `samples/bookstore.sqlite`，写入作者、图书和视图 `book_catalog`，并打开 `books`。

可以接着做这些事：

1. 在左侧展开「表」「视图」和列。
2. 改价格或新增一行，再点「提交」。
3. 点「导出 CSV」。
4. 工具栏「新建查询」，运行 `SELECT title, price FROM books ORDER BY price;`。

## 试用 MySQL / PostgreSQL

本仓库不内置假驱动。没有服务器时，用根目录的 `docker-compose.yml` 起两个库：

```bash
docker compose up -d
```

然后在「新建连接」里填写：

| 引擎 | 主机 | 端口 | 用户 | 密码 | 数据库 |
| --- | --- | --- | --- | --- | --- |
| MySQL | `127.0.0.1` | 3306 | `root` | `dataharbor` | `demo` |
| PostgreSQL | `127.0.0.1` | 5432 | `postgres` | `dataharbor` | `demo` |

先点「测试」，再保存并在左侧双击连接。MariaDB 使用同一套 MySQL 驱动，端口一般也是 3306。需要加密连接时打开 SSL；自签证书再勾选「信任自签证书」。

SSH 在连接时做本地端口转发。HTTP 隧道把流量发给代理的 CONNECT。Redis Cluster 不会用一条 SSH 隧道覆盖所有节点。

首次使用 SSH 前，通过可信渠道核对跳板机指纹，并将正确记录保存到用户 `.ssh/known_hosts`；未知主机、密钥变化和不可读的信任文件都会拒绝连接。单独运行 `ssh-keyscan` 不构成可信核验。

PostgreSQL 和 SQL Server 的隧道 TLS 保留真实证书主机名。MySQL/MariaDB、MongoDB、达梦当前驱动无法为严格 TLS 隧道分别设置连接地址与证书身份，会拒绝相应组合；Redis 和 Oracle 的显式 SSL 选项尚未实现，Snowflake 不支持本机隧道重定向，选择时会明确报错。Oracle 自动分页使用 12c+ 的 OFFSET/FETCH。具体边界见功能对照。

数据同步和传输单次读取上限为 20 万行，超过上限不写入目标；每批独立提交，不能保证整个多批操作原子回滚，操作期间应停止源、目标的其他写入。普通文件导出可以受限导出并显示截断警告。

原生备份优先使用数据库工具；内置回退会完整检查全部表的读取结果，超限则报错。SQLite 回退是表结构和数据脚本，其他 SQL 引擎回退仅为 INSERT 数据脚本，不包含完整对象或一致性快照。界面会明确显示产物类型；完整恢复应使用原生备份并单独验收。

## 连接存在哪里

配置目录：

| 系统 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\dataharbor` |
| macOS | `~/Library/Application Support/dataharbor` |
| Linux | `${XDG_CONFIG_HOME:-~/.config}/dataharbor` |

- `connections.json` 只保存连接资料。密码字段是 `xb1:` 开头的 AES-256-GCM 密文。
- 主密钥首次创建时优先放进系统凭据：服务名 `dataharbor`，账户 `master-key`（Windows 凭据管理器、macOS 钥匙串、Linux Secret Service）。
- 首次使用且没有既有密文时，凭据服务不可用会退回配置目录的 `master.key`（Unix 权限 `0600`，Windows 继承配置目录 ACL）。已有密文时不会因为超时创建替代密钥；文件与系统密钥冲突时仅在已有密文能唯一确认正确候选时继续，否则保留全部资料并要求恢复。
- 第一次启动时，如果本机还留着上一版的配置目录、WebView 数据和系统凭据，程序会在生成新主密钥之前把它们迁过来。新目录里已经有的文件不会被覆盖。迁不走的凭据服务不会挡住启动，`master.key` 仍可解密。
- 编辑连接时密码留空表示沿用原密码；勾选清除才会抹掉。
- 安装系统计划任务时，macOS 的 LaunchAgent 标签是 `com.dataharbor.app.jobs`，Windows 任务名是 `DataHarbor`，Linux 只在 `DATAHARBOR_INSTALL_CRON=1` 时写入 crontab。

## 目录

```
src/                  React 界面：布局、连接、对象树、查询、网格、工具
src-tauri/src/        Rust 后端
  commands.rs         Tauri 命令
  crypto.rs           凭据加密
  store.rs            连接配置
  state.rs            会话、隧道与示例库
  tunnel.rs           SSH 与 HTTP 隧道
  studio.rs           工作台模块入口
  studio/             按导入导出、同步、备份、建模、任务、AI 等领域拆分
  drivers/            SQLite、MySQL、PostgreSQL、MongoDB、Redis、SQL Server、Oracle
docs/FEATURE_ROADMAP.md
```

普通读取查询按引擎追加分页，按需取下一页，不额外重放用户 SQL 计算总数。显式限制、锁定和含写操作的语句按原文执行一次，不自动分页或重试。SQL 导出只接受 SELECT / 只读 CTE 语法；SELECT 中调用的自定义函数仍可能有自身副作用。程序生成的筛选、排序和增删改会对标识符加引号，并检查列名是否属于这张表。

网格和 CSV 的 SQL 修改先完整校验整批语句，再提交到同一会话事务。SQL Server 使用 BEGIN TRANSACTION；Oracle 通过单次 SQL*Plus 事务块执行，单条生成语句超过 8KB 会在写入前拒绝，仍需实机验收。Snowflake SQL API 当前不能保证跨请求会话事务，因此拒绝网格/CSV 原子批量修改，可在查询编辑器执行明确的单条操作。

AI 请求会将用户操作中的问题、SQL、结构说明或待补全单元格内容发送到所配置接口。项目包包含地址、账号、路径和 SQL 等资料；导入的新任务默认禁用，已有同 ID 本地任务不会被覆盖。

## 数据可靠性与使用边界

- 保存快捷键只处理当前表，提交期间禁止重复写入；网格按读取时原值检查并发冲突。SQL 批量冲突会回滚，MongoDB/Redis 仍逐条提交，部分失败会显示已提交条数并要求刷新核对，不能直接重放整批。
- CSV/TXT 空单元格表示空字符串，`\N` 表示 NULL。字面文本 `\N` 写成 `\\N`；仅对反斜杠加 N 的保留值族转义，普通文件路径保持原样。数值按目标字段验证并保留精确文本，不能用空字符串代替数值 NULL。
- 查询结果、翻页、导出和执行计划固定关联原连接与数据库，切换后旧响应失效。查询草稿及执行历史包含 SQL 原文，保存在本机 WebView 存储；关闭查询标签会删除对应恢复草稿，执行历史与草稿分开存储。请避免在草稿中长期保留明文凭据。
- 跨引擎结构同步只自动生成有明确类型映射的变更；无法保证精度、时区或长度等语义时会拒绝并要求人工处理。已有字段类型修改也属于危险变更，需查看 SQL 后明确选择。
- 内置脚本可流式恢复超过 20MB 的总文件，但单条 SQL 仍限 20MB；恢复失败会报告已执行条数。外部 PostgreSQL 普通脚本需要支持 restricted 模式的 psql，自定义归档使用 pg_restore；旧工具会明确拒绝。原生备份工具不能继承隧道、TLS 或特殊认证时不会静默改用不等价连接。
- 系统计划任务会主动建连，实际任务失败返回非零退出码；进程锁防止多个实例同时执行同一批任务。进程崩溃后的外部数据库副作用仍需核对，不能把任务互斥当作全任务原子事务。

验证方法与尚未实机验收的边界见 [docs/TESTING.md](docs/TESTING.md)。
