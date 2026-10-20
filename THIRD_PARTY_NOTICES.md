# 第三方授权说明

DataHarbor 根目录的 [MIT 授权](LICENSE) 适用于本项目自身代码，不会将第三方依赖、字体、系统组件或数据库客户端重新授权为 MIT。第三方组件继续适用其各自的授权条件。

本项目使用的 Inter 与 JetBrains Mono 字体由 `@fontsource/inter` 和 `@fontsource/jetbrains-mono` 提供，采用 SIL Open Font License 1.1（OFL-1.1）。发布包需要保留字体随附的版权声明和授权原文；字体名称、修改和单独销售等条件以其原文为准。生成的授权全文保留这两个包各自的版权声明，不会仅用一个通用 OFL 模板替代。

## 交付文件

- [licenses/THIRD_PARTY_LICENSES.txt](licenses/THIRD_PARTY_LICENSES.txt)：实际安装依赖中收集的授权、NOTICE 和版权原文，相同文本按 SHA-256 去重，每份文本列出对应组件和相对文件名。此文件作为 Tauri 安装包资源一起分发。
- [licenses/DEPENDENCIES.json](licenses/DEPENDENCIES.json)：名称、版本、生态、声明的 license、来源仓库，以及授权文件和需要人工核对的项目。文件不包含本机依赖缓存的绝对路径。
- [scripts/generate-notices.mjs](scripts/generate-notices.mjs)：使用 Node 内置模块运行的离线生成器，不调用 shell、不联网、不安装依赖，也不更新锁文件。

## 生成与更新

先按照 README 准备 Node.js、项目指定的 Rust 工具链以及锁文件对应的依赖。依赖安装或缓存准备可能需要联网；授权生成步骤本身始终离线。随后在项目根目录运行：

```text
npm run licenses
```

也可以直接运行 `node scripts/generate-notices.mjs`。生成器根据 `package-lock.json` 读取本机实际存在的非 dev npm 包；未安装的可选平台包会跳过，必需依赖缺失或版本不符会明确失败。Rust 部分运行 `cargo metadata --offline --locked`，只读取已经缓存的依赖，不执行构建脚本。缺少工具链、缓存或同步的 `Cargo.lock` 时会失败，并保留先前生成的交付文件。

收集范围为依赖包或 crate manifest 根目录的 LICENSE、LICENCE、COPYING、NOTICE、OFL、COPYRIGHT、UNLICENSE、第三方 notices 文件，以及 Cargo 明确声明的 `license_file`。授权文本中的版权内容原样保留，仅统一换行符和末尾换行。输出按固定规则排序，不写入生成时间、本机用户名或绝对缓存路径；同一依赖安装状态下重复生成应得到相同文件。

Cargo 清单包含解析得到的构建依赖和条件平台依赖，因此是便于检查的来源目录，不能据此推断每个发布包都会包含所有条目。不同平台安装的可选 npm 包也可能不同；维护者应在各平台依赖或发布内容变化后重新核对。

## 维护者需要核对的内容

生成器不以 license 标识、链接或通用授权模板伪造缺失的授权原文。未找到文本、声明文件丢失或 metadata 未声明 license 的组件会在生成文本的核对列表和 JSON 的 `needsReview` 中列出，附上已知来源仓库。存在这些条目时，维护者需要从组件原始发布来源核对并补齐所需材料。

此流程不是人工合规审计。维护者仍需核对双重授权的选择、特殊条款、版权归属、NOTICE 要求、包内深层目录中的 vendored 原生代码，以及最终安装包实际包含的内容。外部安装的 WebView、系统库、SQL*Plus、备份工具等也不在此依赖根目录扫描的完整性声明内。更新依赖或制作新版本时，应重新生成、审阅差异，并将本说明和生成文件一同纳入版本管理。
