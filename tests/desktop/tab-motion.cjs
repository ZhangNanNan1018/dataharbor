// Run in the disposable desktop profile used by smoke.cjs.
async page => {
  const invoke = command => page.evaluate(c => window.__TAURI_INTERNALS__.invoke(c), command);
  const info = await invoke("app_info");
  if (!info.configDir.includes("dataharbor-desktop-smoke-") || info.encryption !== "本地密钥文件") {
    throw new Error("Tab motion checks require a disposable desktop profile");
  }
  const sample = await invoke("create_sample_database");
  await page.emulateMedia({ reducedMotion: "no-preference" });
  await page.reload();
  const tree = page.getByRole("tree");
  const label = name => tree.getByText(name, { exact: true });
  for (const name of [sample.group, sample.name, "表"]) await label(name).click();
  await label("books").dblclick();
  await label("authors").dblclick();
  await label("视图").click();
  await label("book_catalog").dblclick();
  const headers = page.locator(".xb-tabs > .ant-tabs-nav .ant-tabs-tab");
  const waitForExits = () => page.waitForFunction(() => !document.querySelector(".xb-tab-exits"));
  await page.getByRole("tabpanel", { name: "book_catalog", exact: true }).waitFor();
  if (await headers.last().evaluate(el => getComputedStyle(el).animationName) !== "xb-tab-enter") throw new Error("Missing entrance animation");

  // Sample within the click, before a fast machine can finish the short animation.
  const closeAndMeasure = index => headers.nth(index).locator(".ant-tabs-tab-remove").evaluate(button => {
    button.click();
    const exits = document.querySelector(".xb-tab-exits");
    return {
      count: document.querySelectorAll(".xb-tabs > .ant-tabs-nav .ant-tabs-tab").length,
      inert: exits?.inert && exits.getAttribute("aria-hidden") === "true",
      slide: document.getAnimations().some(a => a.id === "workspace-tab-slide"),
    };
  });
  const first = await closeAndMeasure(0);
  if (first.count !== 2 || !first.inert || !first.slide) throw new Error("Close should remove the real tab immediately and animate the headers");
  const second = await closeAndMeasure(0);
  if (second.count !== 1) throw new Error("A second close left a real tab behind");
  await waitForExits();
  await label("books").dblclick();
  await label("authors").dblclick();
  await headers.first().click({ button: "right" });
  await page.getByRole("menuitem", { name: "关闭其他标签" }).click();
  if (await headers.count() !== 1) throw new Error("Bulk close retained extra tabs");
  await waitForExits();
  const last = await closeAndMeasure(0);
  if (last.count !== 0 || !last.inert) throw new Error("Last tab should animate over the welcome screen");
  await page.locator(".xb-welcome").waitFor();
  await waitForExits();

  await page.emulateMedia({ reducedMotion: "reduce" });
  await label("books").dblclick();
  if (await headers.first().evaluate(el => getComputedStyle(el).animationName) !== "none") throw new Error("Reduced motion should skip entrance");
  const reduced = await closeAndMeasure(0);
  if (reduced.inert || reduced.slide) throw new Error("Reduced motion should skip exit animations");
  await page.locator(".xb-welcome").waitFor();
  await page.emulateMedia({ reducedMotion: null });
  return "Verified tab entry, sliding, consecutive closes, batch and final close, and reduced motion";
}
