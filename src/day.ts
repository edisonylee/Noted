// All "today" / "now" reasoning uses one resolved IANA time zone. The backend
// owns the persisted preference; this cache keeps first paint and offline phone
// sessions consistent with the last resolved value.
const TIME_ZONE_CACHE_KEY = "noted-resolved-time-zone";

function validTimeZone(value: string | null | undefined): value is string {
  if (!value) return false;
  try {
    new Intl.DateTimeFormat("en", { timeZone: value }).format();
    return true;
  } catch {
    return false;
  }
}

function initialTimeZone(): string {
  try {
    const cached = localStorage.getItem(TIME_ZONE_CACHE_KEY);
    if (validTimeZone(cached)) return cached;
  } catch {
    // Storage can be unavailable in restricted browser contexts.
  }
  const system = Intl.DateTimeFormat().resolvedOptions().timeZone;
  return validTimeZone(system) ? system : "America/New_York";
}

export let APP_TZ = initialTimeZone();

export function configureAppTimeZone(timeZone: string): void {
  if (!validTimeZone(timeZone)) return;
  APP_TZ = timeZone;
  try {
    localStorage.setItem(TIME_ZONE_CACHE_KEY, timeZone);
  } catch {
    // The in-memory setting still applies for this session.
  }
}

// Configured-zone wall-clock parts for an instant (default: now). hour12:false can
// surface "24" at midnight in some engines, so callers mod by 24.
function localParts(d: Date) {
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

// "YYYY-MM-DD" for the given instant in the configured zone. The canonical "today".
export function easternDay(d: Date = new Date()): string {
  const { y, mo, d: dd } = localParts(d);
  return `${y}-${mo}-${dd}`;
}

// Minutes since midnight in the configured zone — for the live "now" marker.
export function easternMinutes(d: Date = new Date()): number {
  const { h, mi } = localParts(d);
  return (Number(h) % 24) * 60 + Number(mi);
}

// Hour 0–23 in the configured zone — for time-of-day greetings.
export function easternHour(d: Date = new Date()): number {
  return Number(localParts(d).h) % 24;
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

// "Today" / "Yesterday" / "Jun 1" (year only if not the current year). Shared by
// the People view and the per-entity page.
export function relativeDay(dateStr: string): string {
  const today = easternDay();
  const diff = dayDiff(today, dateStr);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  const sameYear = dateStr.slice(0, 4) === today.slice(0, 4);
  return formatDay(dateStr, {
    month: "short",
    day: "numeric",
    ...(sameYear ? {} : { year: "numeric" }),
  });
}
