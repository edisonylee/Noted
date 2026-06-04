// All "today" / "now" reasoning runs in the app's fixed timezone (Eastern), so
// days are computed the same way no matter where the user's machine is set —
// and DST (EST↔EDT) is handled automatically by the platform's tz database.
// This mirrors the Rust `APP_TZ` (America/New_York) on the backend.
export const APP_TZ = "America/New_York";

// Eastern wall-clock parts for an instant (default: now). hour12:false can
// surface "24" at midnight in some engines, so callers mod by 24.
function easternParts(d: Date) {
  const p = new Intl.DateTimeFormat("en-CA", {
    timeZone: APP_TZ,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).formatToParts(d);
  const g = (t: string) => p.find((x) => x.type === t)!.value;
  return { y: g("year"), mo: g("month"), d: g("day"), h: g("hour"), mi: g("minute") };
}

// "YYYY-MM-DD" for the given instant in Eastern. The canonical "today".
export function easternDay(d: Date = new Date()): string {
  const { y, mo, d: dd } = easternParts(d);
  return `${y}-${mo}-${dd}`;
}

// Minutes since Eastern midnight — for positioning the live "now" marker.
export function easternMinutes(d: Date = new Date()): number {
  const { h, mi } = easternParts(d);
  return (Number(h) % 24) * 60 + Number(mi);
}

// Hour 0–23 in Eastern — for time-of-day greetings.
export function easternHour(d: Date = new Date()): number {
  return Number(easternParts(d).h) % 24;
}

// Whole-day difference (a − b) between two YYYY-MM-DD strings. Anchored at UTC
// noon so DST transitions never shift the count. Positive = a is later than b.
export function dayDiff(a: string, b: string): number {
  const t = (s: string) => Date.parse(s + "T12:00:00Z");
  return Math.round((t(a) - t(b)) / 86_400_000);
}

// Localized display of a stored YYYY-MM-DD calendar day. Noon anchor keeps it on
// the intended day regardless of the viewer's timezone.
export function formatDay(dateStr: string, opts: Intl.DateTimeFormatOptions): string {
  return new Date(dateStr + "T12:00:00").toLocaleDateString(undefined, opts);
}
