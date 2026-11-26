import { flushSync } from "react-dom";

const slideId = "workspace-tab-slide";
const timing = { duration: 140, easing: "cubic-bezier(0.2, 0, 0.2, 1)" };

/** Commit the close immediately. Only inert labels outlive the closed sessions. */
export function closeWithTabMotion(workspace: HTMLElement | null, ids: string[], close: () => void) {
  const strip = workspace?.querySelector<HTMLElement>(".xb-tabs > .ant-tabs-nav .ant-tabs-nav-wrap");
  if (!workspace || !strip?.animate || !window.matchMedia?.("(prefers-reduced-motion: no-preference)").matches) {
    close();
    return;
  }

  const bounds = strip.getBoundingClientRect();
  const origin = workspace.getBoundingClientRect();
  const exits = document.createElement("div");
  exits.className = "xb-tab-exits";
  exits.inert = true;
  exits.setAttribute("aria-hidden", "true");
  Object.assign(exits.style, {
    left: `${bounds.left - origin.left}px`, top: `${bounds.top - origin.top}px`,
    width: `${bounds.width}px`, height: `${bounds.height}px`,
  });
  const closing = new Set(ids);
  const survivors = new Map<HTMLElement, number>();
  const headers = strip.querySelectorAll<HTMLElement>(".ant-tabs-tab[data-node-key]");
  for (const tab of Array.from(headers)) {
    const rect = tab.getBoundingClientRect();
    // On a second quick close, start from the position currently on screen.
    for (const animation of tab.getAnimations()) {
      if (animation.id === slideId) animation.cancel();
    }
    if (rect.right <= bounds.left || rect.left >= bounds.right) continue;
    if (!closing.has(tab.dataset.nodeKey!)) {
      survivors.set(tab, rect.left);
      continue;
    }
    const ghost = document.createElement("span");
    ghost.className = "xb-tab-exit";
    ghost.textContent = tab.querySelector(".xb-tab-title")?.textContent ?? "";
    const style = getComputedStyle(tab);
    Object.assign(ghost.style, {
      left: `${rect.left - bounds.left}px`, top: `${rect.top - bounds.top}px`,
      width: `${rect.width}px`, height: `${rect.height}px`, padding: style.padding,
      backgroundColor: style.backgroundColor, border: style.border, color: style.color,
    });
    exits.append(ghost);
  }

  flushSync(close);
  if (exits.childElementCount) {
    workspace.append(exits);
    const animations = Array.from(exits.children, (ghost) => ghost.animate([
      { opacity: 1, clipPath: "inset(0 0% 0 0)" },
      { opacity: 0, clipPath: "inset(0 100% 0 0)" },
    ], { ...timing, fill: "forwards" }).finished);
    void Promise.allSettled(animations).then(() => exits.remove());
  }
  for (const [tab, left] of survivors) {
    const dx = left - tab.getBoundingClientRect().left;
    if (tab.isConnected && Math.abs(dx) > 1) {
      tab.animate([{ translate: `${dx}px` }, { translate: "0px" }], { ...timing, id: slideId });
    }
  }
}
