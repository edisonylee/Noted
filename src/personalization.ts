// Greeting copy lives here so the Ask and Home headings stay consistent.
// The preferred name itself is owned by the backend (see `system_settings.rs`,
// which trims and length-checks it) and reaches the UI through
// `usePreferredName`; there is deliberately no separate client-side store.

export function askHeading(displayName: string): string {
  return displayName ? `Hi ${displayName}, ask anything` : "Ask anything";
}

export function homeHeading(displayName: string): string {
  return displayName ? `Hi ${displayName}` : "Welcome back";
}
