import { spawnSync } from "node:child_process";
const generate = process.argv.includes("--generate");
const env = { ...process.env };
delete env.DATAHARBOR_UPDATE_CONTRACTS;
if (generate) env.DATAHARBOR_UPDATE_CONTRACTS = "1";
const result = spawnSync("cargo", ["test", "--lib", "--locked", "contracts::typescript_contracts_match_rust", "--", "--exact"], {
  cwd: new URL("../src-tauri", import.meta.url), env, stdio: "inherit", shell: false,
});
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
