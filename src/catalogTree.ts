/** Tree keys keep the connection identity in the second segment at every catalog level. */
export function belongsToConnection(key: string, connectionId: string): boolean {
  const [kind, id] = key.split("\u001f");
  return kind !== "group" && id === connectionId;
}
