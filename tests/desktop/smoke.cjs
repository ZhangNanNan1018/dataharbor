async page => {
  const info = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("app_info"));
  if (!info.configDir.includes("dataharbor-desktop-smoke-") || info.encryption !== "本地密钥文件") {
    throw new Error("Desktop smoke tests require the disposable DATAHARBOR_TEST_CONFIG_DIR profile.");
  }
  const profiles = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("list_connections"));
  for (const profile of profiles) {
    if (profile.engine !== "sqlite" || !profile.filePath.startsWith(info.configDir)) throw new Error("Unexpected non-fixture connection");
    await page.evaluate(id => window.__TAURI_INTERNALS__.invoke("disconnect", { id }), profile.id);
  }
  await page.reload();
  await page.getByRole("button", { name: "工 具", exact: true }).waitFor();
  if (profiles.length) await page.getByRole("tree").waitFor();
  const sample = page.getByRole("button", { name: "打开示例书店", exact: true });
  if (await sample.count()) {
    await sample.click();
    await page.getByRole("tabpanel", { name: "books", exact: true }).waitFor();
  }
  const newConsole = async () => {
    await page.getByRole("button", { name: "工 具", exact: true }).click();
    await page.getByRole("menuitem", { name: "新建查询", exact: true }).click();
    const panel = page.locator('[role="tabpanel"]:visible');
    await panel.getByRole("textbox", { name: "Editor content", exact: true }).waitFor();
    return await panel.getAttribute("aria-labelledby");
  };
  const run = async sql => {
    const panel = page.locator('[role="tabpanel"]:visible');
    const editor = panel.getByRole("textbox", { name: "Editor content", exact: true });
    await editor.focus();
    await editor.press("Control+A");
    await page.keyboard.insertText(sql);
    await panel.getByRole("button", { name: "运行 SQL", exact: true }).click();
    await panel.getByRole("button", { name: "取消任务", exact: true }).waitFor({ state: "hidden" });
    await panel.getByRole("status", { name: "任务进度" }).getByText(/任务已完成|任务失败/, { exact: true }).waitFor();
    return panel;
  };
  const a = await newConsole();
  await run("CREATE TABLE IF NOT EXISTS smoke_marker(value INTEGER); DELETE FROM smoke_marker; INSERT INTO smoke_marker VALUES(0);");
  const own = await run("BEGIN; UPDATE smoke_marker SET value=77; SELECT value AS own_value FROM smoke_marker;");
  await own.getByRole("cell", { name: "77", exact: true }).waitFor();
  const b = await newConsole();
  const other = await run("SELECT value AS other_value FROM smoke_marker;");
  await other.getByRole("cell", { name: "0", exact: true }).waitFor();
  const refused = await run("COMMIT;");
  await refused.getByText(/no transaction is active/).first().waitFor();
  await page.locator(`[id="${a}"]`).click();
  await run("COMMIT;");
  await page.locator(`[id="${b}"]`).click();
  const committed = await run("SELECT value AS committed_value FROM smoke_marker;");
  await committed.getByRole("cell", { name: "77", exact: true }).waitFor();
  const large = await run("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) SELECT x FROM n;");
  await large.getByTitle("100 条/页", { exact: true }).click();
  await page.getByTitle("500 条/页", { exact: true }).click();
  const viewport = large.locator(".ant-table-tbody-virtual-holder");
  await viewport.waitFor();
  await viewport.evaluate(element => { element.scrollTop = element.scrollHeight; });
  await large.getByText("500", { exact: true }).waitFor();
  const renderedRows = await large.locator(".ant-table-row").count();
  if (renderedRows < 1 || renderedRows > 100) throw new Error("Result virtualization failed");
  const records = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("list_run_history"));
  if (records.length < 6 || !records.some(record => record.status === "failed") || records.some(record => record.status === "running")) {
    throw new Error("Execution history does not match the completed UI operations.");
  }
  await page.getByRole("button", { name: "工 具", exact: true }).click();
  await page.getByRole("menuitem", { name: "定时任务", exact: true }).click();
  await page.getByText("执行记录", { exact: true }).waitFor();
  await page.getByRole("button", { name: "刷新记录", exact: true }).click();
  await page.getByText(records[0].id.slice(0, 8), { exact: true }).waitFor();
  await page.getByText("执行记录", { exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: "desktop-smoke.png", fullPage: true });
  return { configDir: info.configDir, consoleA: a, consoleB: b, records: records.length, renderedRowsFor500: renderedRows, result: "SQLite UI transaction isolation, Monaco input, IPC and execution history passed" };
}
