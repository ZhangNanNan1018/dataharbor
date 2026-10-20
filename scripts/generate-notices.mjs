import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outputDirectory = path.join(root, "licenses");
const compare = (left, right) => left < right ? -1 : left > right ? 1 : 0;
const dependencies = new Map();
const texts = new Map();

function readJson(filename, label) {
  try {
    return JSON.parse(readFileSync(filename, "utf8"));
  } catch {
    throw new Error(`无法读取 ${label}。请先安装与锁文件一致的依赖。`);
  }
}

function declaredLicense(manifest) {
  const value = manifest.license ?? manifest.licenses;
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map((item) => typeof item === "string" ? item : item?.type).filter(Boolean).join(" OR ") || null;
  return value?.type ?? null;
}

function repositoryUrl(value) {
  const text = typeof value === "string" ? value : value?.url;
  if (!text) return null;
  // Never retain a local checkout path or URL credentials in a shipped notice.
  try {
    const url = new URL(text.replace(/^git\+/, ""));
    if (!["https:", "http:", "git:", "ssh:"].includes(url.protocol)) return null;
    url.username = "";
    url.password = "";
    return url.toString();
  } catch {
    return /^[\w.-]+\/[\w.-]+$/.test(text) ? `https://github.com/${text}` : null;
  }
}

function safeRelative(base, filename, label) {
  const relative = path.relative(realpathSync(base), realpathSync(filename));
  if (!relative || relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error(`${label} 的授权文件位于依赖目录之外，无法安全收集。`);
  }
  return relative.split(path.sep).join("/");
}

function isLicenseFilename(filename) {
  return /^(licen[sc]e|copying|notice|ofl|copyright|unlicense|third[-_ ]?party[-_ ]?notices?)(?:$|[-_. ])/i.test(filename)
    || /^thirdpartynotices?(?:\.|$)/i.test(filename);
}

function collectLicenseTexts(base, declaredFile, label) {
  const files = new Set();
  for (const entry of readdirSync(base, { withFileTypes: true })) {
    if (entry.isFile() && isLicenseFilename(entry.name)) files.add(path.join(base, entry.name));
  }
  const problems = [];
  if (declaredFile) {
    const filename = path.resolve(base, declaredFile);
    if (existsSync(filename)) files.add(filename);
    else problems.push("metadata 指定的 license_file 未在已安装依赖中找到");
  }
  const records = [];
  for (const filename of [...files].sort(compare)) {
    const relative = safeRelative(base, filename, label);
    let content;
    try {
      content = readFileSync(filename, "utf8").replace(/\r\n?/g, "\n");
    } catch {
      throw new Error(`无法读取 ${label}/${relative} 的授权原文。`);
    }
    if (!content.trim()) {
      problems.push(`授权文件 ${relative} 为空`);
      continue;
    }
    if (content.includes("\0") || content.includes("\uFFFD")) {
      problems.push(`授权文件 ${relative} 不是可完整读取的 UTF-8 文本，需要人工核对`);
      continue;
    }
    if (!content.endsWith("\n")) content += "\n";
    const sha256 = createHash("sha256").update(content, "utf8").digest("hex");
    const source = `${label}/${relative}`;
    const existing = texts.get(sha256);
    if (existing) existing.sources.add(source);
    else texts.set(sha256, { content, sources: new Set([source]) });
    records.push({ file: relative, sha256 });
  }
  if (!records.length) problems.push("未找到授权原文；metadata 中的 license 不能替代完整授权文本");
  return { records: records.sort((a, b) => compare(a.file, b.file)), problems };
}

function addDependency(ecosystem, manifest, base, declaredFile) {
  const { name, version } = manifest;
  if (!name || !version) throw new Error(`${ecosystem} 依赖缺少名称或版本，无法生成确定的授权目录。`);
  const identity = `${ecosystem}:${name}@${version}`;
  const license = declaredLicense(manifest);
  const collected = collectLicenseTexts(base, declaredFile, identity);
  if (!license) collected.problems.push("依赖 metadata 没有声明 license，需要人工核对来源");
  const entry = {
    ecosystem,
    name,
    version,
    license,
    repository: repositoryUrl(manifest.repository),
    licenseFiles: collected.records,
  };
  if (collected.problems.length) entry.needsReview = collected.problems.sort(compare);
  const previous = dependencies.get(identity);
  if (previous && JSON.stringify(previous) !== JSON.stringify(entry)) {
    throw new Error(`${identity} 的多个已安装副本有不同授权元数据或文本，需要人工核对。`);
  }
  dependencies.set(identity, entry);
}

function collectNpm() {
  const lock = readJson(path.join(root, "package-lock.json"), "package-lock.json");
  if (!lock.packages || lock.lockfileVersion < 2) throw new Error("需要包含 packages 目录的 npm 锁文件（lockfileVersion >= 2）。");
  let installed = 0;
  for (const [location, locked] of Object.entries(lock.packages).sort(([a], [b]) => compare(a, b))) {
    if (!location || locked.dev || !location.startsWith("node_modules/")) continue;
    const directory = path.resolve(root, location);
    if (path.relative(root, directory).startsWith("..")) throw new Error("npm 锁文件包含依赖目录之外的路径。");
    const manifestPath = path.join(directory, "package.json");
    if (!existsSync(manifestPath) && locked.optional) continue;
    const manifest = readJson(manifestPath, `${location}/package.json`);
    if (manifest.version !== locked.version) throw new Error(`${location} 的已安装版本与 package-lock.json 不一致，请重新运行 npm ci。`);
    addDependency("npm", manifest, directory, null);
    installed += 1;
  }
  if (!installed) throw new Error("没有找到已安装的 npm 运行时依赖，请先运行 npm ci。掩盖缺失依赖会产生不完整授权清单。");
}

function collectCargo() {
  const result = spawnSync("cargo", ["metadata", "--manifest-path", path.join(root, "src-tauri", "Cargo.toml"), "--offline", "--locked", "--format-version", "1"], {
    cwd: root,
    shell: false,
    windowsHide: true,
    // Cargo's --offline does not govern rustup's toolchain auto-installation.
    env: { ...process.env, RUSTUP_AUTO_INSTALL: "0" },
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    timeout: 60_000,
  });
  if (result.error?.code === "ENOENT") throw new Error("未找到 cargo。请安装项目指定的 Rust 工具链，并确保 cargo 位于 PATH。");
  if (result.error || result.status !== 0) {
    const diagnostic = result.stderr ?? "";
    const missingCrate = diagnostic.match(/failed to download `([A-Za-z0-9_.+-]+ v[0-9A-Za-z_.+-]+)`/);
    let hint = "请先准备已缓存的 Rust 依赖和同步的 Cargo.lock。";
    if (missingCrate) hint = `本机缺少已缓存的 ${missingCrate[1]}；请先在依赖准备步骤运行 cargo fetch --locked。`;
    else if (/Missing manifest in toolchain|toolchain .* is not installed|rustc.*not found/i.test(diagnostic)) hint = "项目指定的 Rust 工具链缺失或损坏，请先修复工具链；已有兼容工具链可通过 RUSTUP_TOOLCHAIN 选择。";
    else if (/lock file.*needs to be updated|lockfile.*needs to be updated|--locked/i.test(diagnostic)) hint = "Cargo.lock 需要与 Cargo.toml 同步，请先在依赖准备步骤更新并核对锁文件。";
    throw new Error(`离线 cargo metadata 失败（${result.error?.code ?? `退出码 ${result.status}`}）。${hint}可手动运行 cargo metadata --offline --locked 查看诊断。生成器不会联网或更新锁文件。`);
  }
  let metadata;
  try { metadata = JSON.parse(result.stdout); }
  catch { throw new Error("cargo metadata 未返回有效 JSON，无法生成 Rust 授权目录。"); }
  const workspace = new Set(metadata.workspace_members);
  for (const manifest of metadata.packages.sort((a, b) => compare(a.id, b.id))) {
    if (workspace.has(manifest.id)) continue;
    addDependency("cargo", manifest, path.dirname(manifest.manifest_path), manifest.license_file);
  }
}

function writeOutputs() {
  const entries = [...dependencies.values()].sort((a, b) => compare(`${a.ecosystem}:${a.name}@${a.version}`, `${b.ecosystem}:${b.name}@${b.version}`));
  const review = entries.filter((entry) => entry.needsReview);
  const notice = [
    "DataHarbor — third-party license texts",
    "Generated offline by scripts/generate-notices.mjs. No network requests or dependency code execution.",
    "Scope: installed non-dev npm packages from package-lock.json; all resolved third-party Cargo packages from cargo metadata --offline --locked (including build and platform-conditional dependencies).",
    "This is a source-text inventory, not a legal audit or a claim that every listed crate is shipped in every build. Review vendored native code and target-specific release contents separately.",
    "Original copyright statements are retained. Only line endings and the final newline are normalized. Identical texts are grouped by SHA-256.",
    "",
    "DEPENDENCY DIRECTORY",
    ...entries.map((entry) => `${entry.ecosystem}:${entry.name}@${entry.version} | declared license: ${entry.license ?? "NOT DECLARED"} | repository: ${entry.repository ?? "NOT DECLARED"}`),
    "",
    "SOURCE TEXTS REQUIRING MANUAL REVIEW",
    ...(review.length ? review.flatMap((entry) => [`${entry.ecosystem}:${entry.name}@${entry.version}`, `  declared license: ${entry.license ?? "NOT DECLARED"}`, `  repository: ${entry.repository ?? "NOT DECLARED"}`, ...entry.needsReview.map((reason) => `  - ${reason}`)]) : ["No missing source texts were detected by the configured root-file scan. Maintainer review is still required."]),
    "",
    "LICENSE AND NOTICE TEXTS",
  ];
  for (const [sha256, record] of [...texts.entries()].sort(([a], [b]) => compare(a, b))) {
    notice.push("", `===== SHA-256 ${sha256} =====`, "Sources:", ...[...record.sources].sort(compare).map((source) => `  ${source}`), "", record.content);
  }
  mkdirSync(outputDirectory, { recursive: true });
  writeFileSync(path.join(outputDirectory, "THIRD_PARTY_LICENSES.txt"), `${notice.join("\n")}\n`, "utf8");
  writeFileSync(path.join(outputDirectory, "DEPENDENCIES.json"), `${JSON.stringify({ formatVersion: 1, dependencies: entries }, null, 2)}\n`, "utf8");
  process.stdout.write(`已生成 ${entries.length} 项依赖、${texts.size} 份去重授权文本；${review.length} 项需要人工核对。输出：licenses/THIRD_PARTY_LICENSES.txt、licenses/DEPENDENCIES.json\n`);
}

try {
  collectNpm();
  collectCargo();
  writeOutputs();
} catch (error) {
  process.stderr.write(`第三方授权生成失败：${error.message}\n`);
  process.exitCode = 1;
}
