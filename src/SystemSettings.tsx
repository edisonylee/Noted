import { useEffect, useState } from "react";
import { api, type SystemSettings } from "./api";
import { configureAppTimeZone } from "./day";

const COMMON_TIME_ZONES = [
  ["America/Los_Angeles", "Pacific Time (Los Angeles)"],
  ["America/Denver", "Mountain Time (Denver)"],
  ["America/Chicago", "Central Time (Chicago)"],
  ["America/New_York", "Eastern Time (New York)"],
  ["America/Anchorage", "Alaska Time (Anchorage)"],
  ["Pacific/Honolulu", "Hawaii Time (Honolulu)"],
] as const;

type IntlWithSupportedValues = typeof Intl & {
  supportedValuesOf?: (key: "timeZone") => string[];
};

const ALL_TIME_ZONES =
  (Intl as IntlWithSupportedValues).supportedValuesOf?.("timeZone") ??
  COMMON_TIME_ZONES.map(([value]) => value);
const COMMON_TIME_ZONE_NAMES = new Set<string>(COMMON_TIME_ZONES.map(([value]) => value));

function timeZoneName(value: string): string {
  const common = COMMON_TIME_ZONES.find(([timeZone]) => timeZone === value);
  if (common) return common[1];
  return value.replace(/_/g, " ").replace(/\//g, " · ");
}

function timeZonePreview(value: string): string {
  try {
    return new Intl.DateTimeFormat(undefined, {
      weekday: "long",
      month: "long",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
      timeZoneName: "short",
      timeZone: value,
    }).format(new Date());
  } catch {
    return value;
  }
}

export function SystemSettingsPanel() {
  const [settings, setSettings] = useState<SystemSettings | null>(null);
  const [timeZonePreference, setTimeZonePreference] = useState("system");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.systemSettingsGet()
      .then((next) => {
        setSettings(next);
        setTimeZonePreference(next.timeZone);
      })
      .catch((reason) => setError(String(reason)));
  }, []);

  async function saveTimeZone(nextPreference: string) {
    setTimeZonePreference(nextPreference);
    setSaving(true);
    setError(null);
    try {
      const next = await api.systemSettingsSet(nextPreference);
      setSettings(next);
      configureAppTimeZone(next.resolvedTimeZone);
      window.location.reload();
    } catch (reason) {
      setError(String(reason));
      setSaving(false);
    }
  }

  return (
    <>
      <h3>System</h3>
      <p className="settings-sub">
        Set the calendar day and wall clock Noted uses for schedules, journals, captures,
        greetings, and connected calendars.
      </p>

      <div className="settings-fields system-settings-fields">
        <section className="settings-group">
          <header className="settings-group-head">
            <h4>Time zone</h4>
            <p>
              Changing the weather city sets this automatically. Choose “Use this Mac’s time zone”
              to follow macOS again.
            </p>
          </header>
          <label className="field">
            <span className="field-label">Calendar and schedule time zone</span>
            <select
              value={timeZonePreference}
              onChange={(event) => {
                void saveTimeZone(event.target.value);
              }}
              disabled={!settings || saving}
            >
              <option value="system">
                Use this Mac’s time zone{settings ? ` — ${timeZoneName(settings.systemTimeZone)}` : ""}
              </option>
              {timeZonePreference !== "system" &&
                !COMMON_TIME_ZONE_NAMES.has(timeZonePreference) &&
                !ALL_TIME_ZONES.includes(timeZonePreference) && (
                  <option value={timeZonePreference}>{timeZoneName(timeZonePreference)}</option>
                )}
              <optgroup label="United States">
                {COMMON_TIME_ZONES.map(([value, label]) => (
                  <option key={value} value={value}>{label}</option>
                ))}
              </optgroup>
              <optgroup label="All time zones">
                {ALL_TIME_ZONES.filter((value) => !COMMON_TIME_ZONE_NAMES.has(value)).map((value) => (
                  <option key={value} value={value}>{timeZoneName(value)}</option>
                ))}
              </optgroup>
            </select>
            {settings && (
              <span className="field-hint">
                {saving ? "Saving time zone…" : <>Current Noted time: {timeZonePreview(
                  timeZonePreference === "system" ? settings.systemTimeZone : timeZonePreference
                )}</>}
              </span>
            )}
          </label>
        </section>
      </div>

      {error && <div className="settings-actions"><span className="field-hint settings-error">{error}</span></div>}
    </>
  );
}
