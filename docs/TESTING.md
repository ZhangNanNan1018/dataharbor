# 验证方法与覆盖边界

## 本地回归

```sh
npm ci
npm run build
cd src-tauri
cargo test --lib --locked
```

测试使用临时配置目录和数据库，强制文件测试密钥，不读取个人系统凭据或已保存连接。覆盖 SQLite 的查询、分页、改行、事务回滚、CSV、同步主键、读取截断、内置脚本和项目导入；SSH/TLS 的配置与信任校验使用本地夹具。

## 专用服务器往返测试

根目录 `compose.test.yml` 创建独立数据库，默认仅在本机端口 13306 和 15432 监听，账号密码仅用于测试。请勿把测试端口指向业务数据库。

PowerShell：

```powershell
docker compose -p dataharbor-tests -f compose.test.yml up -d --wait
$env:DATAHARBOR_TEST_MYSQL_PORT = '13306'
$env:DATAHARBOR_TEST_POSTGRES_PORT = '15432'
Set-Location src-tauri
cargo test --lib --locked server_tests:: -- --ignored --test-threads=1
Set-Location ..
docker compose -p dataharbor-tests -f compose.test.yml down -v
```

这两项显式测试分别在 MySQL 8.4、PostgreSQL 16 验证真实连接、复合主键元数据、读取与修改、失败事务回滚、CSV 导出后重新导入，并读取数据库核对结果。测试表名随机，测试结束只清理自己创建的表和临时文件。

GitHub CI 在三个桌面系统执行构建与回归，在独立 Linux 服务容器执行上述服务器测试。工作流配置存在不代表已经在 GitHub 运行通过。

2026-10-20 本机验证：Windows、Node.js 24.15.0、Rust 1.94.0，前端构建通过；默认 Rust 回归 71 项通过，另外两项专用服务器测试在 MySQL 8.4、PostgreSQL 16 容器中通过。服务器测试还用真实 PostgreSQL sequence 验证带函数的查询不会为自动计数被额外执行。后续修改应重新核对该记录。

## 发布验收记录

维护者需要逐个平台和引擎记录版本、构建、启动与主流程结果。当前自动化不能证明下列情况已经验证：

- macOS/Linux 的实际安装和桌面窗口交互；
- SQL Server、Oracle、达梦、Snowflake、MongoDB、Redis 的服务器端端到端验收；
- 真实跳板机、证书链、代理、集群故障切换；
- 多用户并发写入时的一致性快照或全任务原子回滚。

后台任务的配置锁和运行锁为进程内保护。不要让多个应用实例或外部 `--run-jobs` 进程同时操作同一配置目录；跨进程任务租约尚未实现。

修改相关模块后运行对应回归，再执行整体构建。不得把未运行或仅静态检查的项记录成通过。
