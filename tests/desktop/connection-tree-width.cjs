// Run in the same disposable Tauri profile as the other desktop smoke tests.
async page => {
  const invoke = (command, args) => page.evaluate(([c, a]) => window.__TAURI_INTERNALS__.invoke(c, a), [command, args]);
  const info = await invoke("app_info");
  if (!info.configDir.includes("dataharbor-desktop-smoke-") || info.encryption !== "本地密钥文件") {
    throw new Error("Connection tree sizing requires a disposable profile");
  }
  const group = "连接树宽度验证";
  const longName = "业务数据分析与客户服务历史记录连接";
  const table = "ai_agent_attachment_parse_customer_report_history";
  const saved = [];
  try {
    for (const profile of [
      { name: "agent-semantic", engine: "mysql", group, host: "127.0.0.1", username: "fixture", port: 3306 },
      { name: longName, engine: "sqlite", group: group + "/数据平台", filePath: info.configDir + "/width.sqlite" },
    ]) {
      const { connection } = await invoke("save_connection", { request: profile });
      saved.push(connection.id);
    }
    await invoke("connect", { id: saved[1] });
    const result = await invoke("execute_sql", { request: {
      connectionId: saved[1], database: "main", sql: "CREATE TABLE IF NOT EXISTS " + table + " (id INTEGER)",
    } });
    if (result.stopped) throw new Error(result.statements[0].error);
    await page.reload();
    const sidebar = page.locator(".xb-sidebar");
    const tree = sidebar.getByRole("tree");
    const label = name => tree.getByText(name, { exact: true });
    for (const name of [group, "数据平台", longName, "表"]) await label(name).click();
    await label(table).waitFor();
    const fields = await tree.locator(".xb-conn").filter({ hasText: longName }).locator(":scope > *").allTextContents();
    if (fields.length !== 3 || fields[1] !== longName || fields[2] !== "SQLite") throw new Error("Unexpected connection labels");

    const viewport = sidebar.locator(".overflow-auto");
    const handle = page.locator(".xb-resize");
    for (const width of [220, 292, 480]) {
      const grip = await handle.boundingBox();
      const current = await sidebar.evaluate(el => el.getBoundingClientRect().width);
      const x = grip.x + grip.width / 2, y = grip.y + grip.height / 2;
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x + width - current, y);
      await page.mouse.up();
      await page.waitForFunction(w => Math.abs(document.querySelector(".xb-sidebar").getBoundingClientRect().width - w) < 1, width);
      for (const name of [group, "agent-semantic", longName, table]) {
        const height = await label(name).evaluate(el => el.closest(".ant-tree-node-content-wrapper").getBoundingClientRect().height);
        if (height !== 28) throw new Error(name + " wrapped at " + width);
      }
      const positions = () => sidebar.locator("input[placeholder], :scope > div:first-child").evaluateAll(els => els.map(el => el.getBoundingClientRect().left));
      const before = await positions();
      const scrolled = await viewport.evaluate(el => { el.scrollLeft = el.scrollWidth; return el.scrollLeft; });
      if ((scrolled > 0) !== (width < 480)) throw new Error("Incorrect overflow at " + width);
      const end = await label(table).evaluate(el => {
        const range = document.createRange();
        range.selectNodeContents(el);
        return range.getBoundingClientRect().right;
      });
      const right = await viewport.evaluate(el => el.getBoundingClientRect().left + el.clientWidth);
      if (end > right + 1) throw new Error("Table name is clipped at " + width);
      if (JSON.stringify(before) !== JSON.stringify(await positions())) throw new Error("Sidebar header scrolled");
      await viewport.evaluate(el => { el.scrollLeft = 0; });
    }
    return "Verified single-line labels and tree-only horizontal scrolling at 220, 292 and 480px";
  } finally {
    for (const id of saved) await invoke("delete_connection", { id });
  }
}
