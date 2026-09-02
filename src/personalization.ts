const DISPLAY_NAME_KEY = "noted-display-name-v1";

export function normalizeDisplayName(value: string): string {
  return value.trim().replace(/\s+/g, " ").slice(0, 80);
}

export function readDisplayName(): string {
  try {
    return normalizeDisplayName(localStorage.getItem(DISPLAY_NAME_KEY) ?? "");
  } catch {
    return "";
  }
}

export function writeDisplayName(value: string): string {
  const normalized = normalizeDisplayName(value);
  try {
    if (normalized) localStorage.setItem(DISPLAY_NAME_KEY, normalized);
    else localStorage.removeItem(DISPLAY_NAME_KEY);
  } catch {
    // The generic greeting remains correct when local storage is unavailable.
  }
  return normalized;
}

export function askHeading(displayName: string): string {
  return displayName ? `Hi ${displayName}, ask anything` : "Ask anything";
}

export function homeHeading(displayName: string): string {
  return displayName ? `Hi ${displayName}` : "Welcome back";
}
