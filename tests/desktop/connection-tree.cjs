// Run with playwright-cli against the disposable desktop profile used by smoke.cjs.
async page => {
  const invoke = command => page.evaluate(c => window.__TAURI_INTERNALS__.invoke(c), command);
  const info = await invoke("app_info");
  if (!info.configDir.includes("dataharbor-desktop-smoke-") || info.encryption !== "本地密钥文件") {
    throw new Error("The connection tree check requires a disposable desktop profile");
  }
  const sample = await invoke("create_sample_database");
  await page.reload();
  const tree = page.getByRole("tree");
  const label = name => tree.getByText(name, { exact: true });
  const row = name => label(name).locator('xpath=ancestor::*[@role="treeitem"]');
  const body = name => row(name).locator(".ant-tree-node-content-wrapper");
  const arrow = name => row(name).locator(".ant-tree-switcher");
  async function expanded(name, value) {
    await row(name).and(tree.locator('[aria-expanded="' + value + '"]')).waitFor();
  }
  async function box(locator) {
    await locator.waitFor();
    const rect = await locator.boundingBox();
    if (!rect) throw new Error("Missing row bounds");
    return rect;
  }
  const group = sample.group;
  await label(group).click();
  await label(sample.name).click();
  await label("表").click();
  await label("books").waitFor();
  const initial = await page.evaluate(() => localStorage.getItem("xb-theme") || "system");
  async function theme(value) {
    await page.getByRole("button", { name: "切换主题" }).click();
    await page.getByRole("menuitem", { name: { light: "明亮", dark: "暗色", system: "跟随系统" }[value] }).click();
  }
  for (const mode of ["light", "dark"]) {
    await theme(mode);
    await page.waitForFunction(value => document.documentElement.dataset.theme === value, mode);
    // Select the folder first so the table starts unselected.
    await label("表").click();
    await expanded("表", false);
    await label("表").click();
    await label("books").waitFor();
    const content = body("books");
    const colors = await content.evaluate(el => {
      const probe = document.createElement("span");
      el.append(probe);
      const colors = ["--xb-hover", "--xb-accent-soft"].map(token => {
        probe.style.backgroundColor = "var(" + token + ")";
        return getComputedStyle(probe).backgroundColor;
      });
      probe.remove();
      return colors;
    });
    const waitForColor = color => page.waitForFunction(expected => {
      const el = [...document.querySelectorAll(".xb-tree .ant-tree-node-content-wrapper")].find(node => node.textContent.trim() === "books");
      return el && getComputedStyle(el).backgroundColor === expected;
    }, color);
    await content.hover();
    const arrowBox = await box(arrow("books"));
    const rowBox = await box(content);
    const y = rowBox.y + rowBox.height / 2;
    const arrowX = arrowBox.x + arrowBox.width / 2;
    const labelX = (await box(label("books"))).x + 3;
    const trailingX = rowBox.x + rowBox.width - 4;
    if (Math.abs(arrowBox.x - rowBox.x) > 1 || rowBox.height !== 28) throw new Error("Hit area must begin at the arrow and retain row height");
    for (const x of [arrowX, labelX, trailingX]) {
      await page.mouse.move(x, y);
      const hit = await content.evaluate((el, p) => el.contains(document.elementFromPoint(p.x, p.y)) && el.matches(":hover"), { x, y });
      if (!hit) throw new Error("Arrow, text and trailing space must hover the same row");
      await waitForColor(colors[0]);
    }
    await page.mouse.move(arrowBox.x - 5, y);
    if (await content.evaluate(el => el.matches(":hover"))) throw new Error("Indent should stay outside the hit area");
    await page.mouse.click(arrowX, y);
    if (await row("books").getAttribute("aria-expanded") !== null) throw new Error("Tables must be leaves");
    await row("books").and(tree.locator(".ant-tree-treenode-selected")).waitFor();
    await waitForColor(colors[1]);
    await page.mouse.click(trailingX, y);
    if (await arrow("books").locator("svg").count()) throw new Error("Table showed an expand arrow");
    await row("books").and(tree.locator(".ant-tree-treenode-selected")).waitFor();
    await page.mouse.move(labelX, y);
    await page.mouse.down();
    await page.mouse.move(trailingX, y, { steps: 6 });
    await page.mouse.up();
    if (await page.evaluate(() => !!window.getSelection()?.toString())) throw new Error("Dragging a tree label selected text");
    await label("books").click({ button: "right" });
    await page.getByRole("button", { name: "查看结构", exact: true }).waitFor();
    await page.locator(".xb-sidebar input").first().click();
    const keyboard = page.locator(".xb-tree input");
    await keyboard.focus();
    await keyboard.press("ArrowDown");
    await row(group).and(tree.locator(".ant-tree-treenode-active")).waitFor();
    await keyboard.press("ArrowLeft");
    await expanded(group, false);
    await keyboard.press("ArrowRight");
    await expanded(group, true);
    const focus = await body(group).evaluate(el => getComputedStyle(el).outlineStyle);
    if (focus === "none") throw new Error("Keyboard navigation must retain its row outline");
  }
  await theme(initial);
  await label("books").dblclick();
  await page.getByRole("tabpanel", { name: "books", exact: true }).waitFor();
  if (await row("books").getAttribute("aria-expanded") !== null) throw new Error("Tables must be leaves");
  await label("视图").click();
  await label("book_catalog").click();
  if (await row("book_catalog").getAttribute("aria-expanded") !== null || await arrow("book_catalog").locator("svg").count()) throw new Error("Views must be leaves");
  await label("book_catalog").click({ button: "right" });
  await page.getByRole("button", { name: "查看结构", exact: true }).waitFor();
  await page.locator(".xb-sidebar input").first().click();
  await label("book_catalog").dblclick();
  await page.getByRole("tabpanel", { name: "book_catalog", exact: true }).waitFor();
  return "Verified table and view leaves, context menus, double-click, row highlights and keyboard navigation";
}
