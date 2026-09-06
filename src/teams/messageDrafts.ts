export function draftKey(org: string, user: string) {
  return `noted:message-drafts:v1:${encodeURIComponent(org)}:${encodeURIComponent(user)}`;
}
export function readDrafts(key: string): Record<string, string> {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(key) ?? "{}");
    if (!value || typeof value !== "object" || Array.isArray(value)) return {};
    return Object.fromEntries(Object.entries(value).filter(([, body]) => typeof body === "string"));
  } catch { return {}; }
}
export function writeDrafts(key: string, drafts: Record<string, string>) {
  try {
    localStorage.setItem(key, JSON.stringify(Object.fromEntries(Object.entries(drafts).filter(([, body]) => body.length))));
    return true;
  } catch { return false; }
}
