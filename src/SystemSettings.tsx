import { save } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type SystemSettings } from "./api";
import { configureAppTimeZone } from "./day";
import { configurePreferredName } from "./usePreferredName";
import { AppUpdateSettings } from "./AppUpdateSettings";

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
  const [backupMsg, setBackupMsg] = useState("");
  const [backingUp, setBackingUp] = useState(false);
  async function onBackup() {
    setBackingUp(true);
    setBackupMsg("");
    try {
      const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
      const destination = await save({ defaultPath: `noted-backup-${timestamp}.db`, filters: [{ name: "Noted database", extensions: ["db"] }] });
      if (!destination) return;
      setBackupMsg(`Backed up to ${await api.exportDb(destination)}`);
    }
    catch (reason) { setBackupMsg(`Backup failed: ${reason}`); }
    finally { setBackingUp(false); }
  }
  const [settings, setSettings] = useState<SystemSettings | null>(null);
  const [timeZonePreference, setTimeZonePreference] = useState("system");
  const [preferredName, setPreferredName] = useState("");
  const [saving, setSaving] = useState(false);
  const [nameSaving, setNameSaving] = useState(false);
  const [nameSaved, setNameSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.systemSettingsGet()
      .then((next) => {
        setSettings(next);
        setTimeZonePreference(next.timeZone);
        setPreferredName(next.preferredName ?? "");
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

  async function savePreferredName() {
    if (!settings || nameSaving) return;
    setNameSaving(true);
    setNameSaved(false);
    setError(null);
    try {
      const next = await api.systemSettingsSet(settings.timeZone, preferredName);
      setSettings(next);
      setPreferredName(next.preferredName ?? "");
      configurePreferredName(next.preferredName);
      setNameSaved(true);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setNameSaving(false);
    }
  }

  return (
    <>
      <h3>General</h3>
      <p className="settings-sub">
        Personalize Noted and set the calendar day and wall clock used for schedules,
        journals, captures, greetings, and connected calendars.
      </p>

      <div className="settings-fields system-settings-fields">
        <section className="settings-group">
          <header className="settings-group-head">
            <h4>Personalization</h4>
            <p>
              Optionally choose how Noted greets you. This stays in your local settings.
            </p>
          </header>
          <form
            className="preferred-name-form"
            onSubmit={(event) => {
              event.preventDefault();
              void savePreferredName();
            }}
          >
            <label className="field">
              <span className="field-label">Preferred name</span>
              <div className="preferred-name-row">
                <input
                  value={preferredName}
                  onChange={(event) => {
                    setPreferredName(event.target.value);
                    setNameSaved(false);
                  }}
                  placeholder="What should Noted call you?"
                  autoComplete="name"
                  maxLength={80}
                  disabled={!settings || nameSaving}
                />
                <button
                  className="ghost-btn"
                  type="submit"
                  disabled={
                    !settings ||
                    nameSaving ||
                    preferredName.trim() === (settings.preferredName ?? "")
                  }
                >
                  {nameSaving ? "Saving…" : "Save"}
                </button>
              </div>
              <span className="field-hint">
                {nameSaved
                  ? "Preferred name saved."
                  : "Leave this blank for a neutral greeting."}
              </span>
            </label>
          </form>
        </section>
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
        <section className="settings-group">
          <header className="settings-group-head"><h4>Database backup</h4><p>Export a copy of your Noted database to this Mac.</p></header>
          <button className="ghost" onClick={() => void onBackup()} disabled={backingUp}>{backingUp ? "Backing up…" : "Back up database"}</button>
          {backupMsg && <p className="field-hint" role="status" style={{ overflowWrap: "anywhere" }}>{backupMsg}</p>}
        </section>
        <AppUpdateSettings />
      </div>

      {error && <div className="settings-actions"><span className="field-hint settings-error">{error}</span></div>}
    </>
  );
}
