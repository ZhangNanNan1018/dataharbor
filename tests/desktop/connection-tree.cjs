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
  const initialTheme = await page.locator("html").getAttribute("data-theme");
  for (let i = 0; i < 2; i++) {
    // Start each theme with an unselected, collapsed table.
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
    await expanded("books", true);
    await row("books").and(tree.locator(".ant-tree-treenode-selected")).waitFor();
    await waitForColor(colors[1]);
    await page.mouse.click(trailingX, y);
    await expanded("books", false);
    await row("books").and(tree.locator(".ant-tree-treenode-selected")).waitFor();
    await label("books").click({ button: "right" });
    await page.getByRole("button", { name: "打开数据", exact: true }).waitFor();
    await expanded("books", false);
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
    await page.getByRole("button", { name: "切换主题" }).click();
  }
  if (await page.locator("html").getAttribute("data-theme") !== initialTheme) throw new Error("Theme did not restore");
  await label("books").dblclick();
  await page.getByRole("tabpanel", { name: "books", exact: true }).waitFor();
  await expanded("books", true);
  return "Verified tree hit areas, selection, expansion, double-click and keyboard navigation in the desktop app";
}
