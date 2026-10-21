const KEY = "xb-recent-connections";

export function touchRecent(id: string) {
  try {
    const prev = read();
    const next = [id, ...prev.filter((item) => item !== id)].slice(0, 12);
    localStorage.setItem(KEY, JSON.stringify(next));
  } catch {
    /* Recent connections are a convenience; a full store must not block connecting. */
  }
}

export function recentRank(id: string): number {
  const index = read().indexOf(id);
  return index === -1 ? 999 : index;
}

export function hasRecent(): boolean {
  return read().length > 0;
}

function read(): string[] {
  try {
    const parsed = JSON.parse(localStorage.getItem(KEY) || "[]") as unknown;
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
}
