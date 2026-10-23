# 参与 DataHarbor

当前版本为早期预览。先阅读 [README](README.md)、[功能范围](docs/FEATURE_ROADMAP.md) 和 [模块边界](docs/ARCHITECTURE.md)。

## 提交前验证

使用 Node.js 20+（CI 使用 24）、Rust 1.94.0，以及 README 所列系统依赖：

```sh
npm ci
npm run build
cd src-tauri
cargo test --lib --locked
```

涉及数据库读写时，按 [测试说明](docs/TESTING.md) 运行专用 MySQL/PostgreSQL 容器中的往返测试。不得使用个人保存连接、业务数据库或生产凭据作为测试夹具。

修复数据写入、备份、导入、连接信任等问题时，添加能复现原错误的回归测试，并检查失败前后数据库状态；只验证返回成功不足以证明修复完成。界面改动需检查处理中、失败、取消、空结果及重复点击。

依赖变动后运行 `cargo fetch --locked` 和 `npm run licenses`，将更新的锁文件和 `licenses/` 内容一起提交。发布安装包前再次生成授权信息。

## 修改范围

- 优先复用领域模块；Tauri 命令只负责边界转换，业务规则放到负责该规则的模块。
- 保留中文错误和明确的限制；不支持的协议选项应拒绝，不能静默退化。
- 数据不完整、主键不明确、预览已失效时停止破坏性操作。
- 提交信息使用简洁的简体中文，例如 `修复: 保留同步复合主键`。

报告问题时说明应用版本、系统、数据库及版本、脱敏步骤、预期与实际行为。连接密码、密钥、业务 SQL、项目包和数据库文件不应直接附到公开 Issue。

## 发布

CI 检查与安装包工作流见 `.github/workflows/`。手动触发的安装包工作流仅上传预览产物，不发布 GitHub Release。上传前应人工确认授权、平台启动、数据库验证矩阵及已知限制，正式分发另行配置代码签名和 macOS 公证。


修改 Rust 传输类型、命令或引擎能力后，运行 `npm run contracts:generate` 并提交 `src/generated/contracts.ts`。`npm run check` 执行前端回归、生产构建和契约检查；完整数据库回归仍按测试说明运行。CI 保持手动触发，普通 Rust 测试包含契约一致性检查。

Windows 真实桌面检查脚本为 `tests/desktop/smoke.cjs`，需要使用测试说明中的独立配置及 WebView 目录。测试脚本会修改示例 SQLite，不能连接用户保存的业务连接。
