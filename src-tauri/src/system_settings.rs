use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

const CONFIG_FILE: &str = "system-settings.json";
const SYSTEM_TIME_ZONE: &str = "system";
const FALLBACK_TIME_ZONE: &str = "America/New_York";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredSettings {
    #[serde(default = "default_time_zone")]
    time_zone: String,
    #[serde(default)]
    preferred_name: Option<String>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            time_zone: default_time_zone(),
            preferred_name: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemSettings {
    pub time_zone: String,
    pub resolved_time_zone: String,
    pub system_time_zone: String,
    pub preferred_name: Option<String>,
}

static SETTINGS: OnceLock<RwLock<StoredSettings>> = OnceLock::new();
static ACTIVE_TIME_ZONE: OnceLock<RwLock<chrono_tz::Tz>> = OnceLock::new();

fn default_time_zone() -> String {
    SYSTEM_TIME_ZONE.to_string()
}

fn settings_cell() -> &'static RwLock<StoredSettings> {
    SETTINGS.get_or_init(|| RwLock::new(StoredSettings::default()))
}

fn time_zone_cell() -> &'static RwLock<chrono_tz::Tz> {
    ACTIVE_TIME_ZONE.get_or_init(|| {
        RwLock::new(
            FALLBACK_TIME_ZONE
                .parse()
                .expect("the fallback time zone must be valid"),
        )
    })
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

fn parse_time_zone(name: &str) -> Option<chrono_tz::Tz> {
    name.trim().parse::<chrono_tz::Tz>().ok()
}

fn zone_name_from_localtime(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    let name = text.split("/zoneinfo/").nth(1)?.trim();
    parse_time_zone(name).map(|_| name.to_string())
}

/// Resolve the Mac's IANA zone without shelling out. On macOS `/etc/localtime`
/// points into `/var/db/timezone/zoneinfo`; the other fallbacks keep local dev
/// and tests useful on Linux as well.
pub fn system_time_zone_name() -> String {
    if let Ok(name) = std::env::var("TZ") {
        if parse_time_zone(&name).is_some() {
            return name;
        }
    }
    if let Ok(path) = std::fs::read_link("/etc/localtime") {
        if let Some(name) = zone_name_from_localtime(&path) {
            return name;
        }
    }
    if let Ok(name) = std::fs::read_to_string("/etc/timezone") {
        let name = name.trim();
        if parse_time_zone(name).is_some() {
            return name.to_string();
        }
    }
    FALLBACK_TIME_ZONE.to_string()
}

fn resolve_time_zone(preference: &str) -> Result<(String, chrono_tz::Tz)> {
    let name = if preference == SYSTEM_TIME_ZONE {
        system_time_zone_name()
    } else {
        preference.trim().to_string()
    };
    let zone = parse_time_zone(&name).ok_or_else(|| anyhow!("unknown time zone: {name}"))?;
    Ok((name, zone))
}

fn snapshot() -> SystemSettings {
    let stored = settings_cell().read().unwrap().clone();
    let system = system_time_zone_name();
    let resolved = resolve_time_zone(&stored.time_zone)
        .map(|(name, _)| name)
        .unwrap_or_else(|_| system.clone());
    SystemSettings {
        time_zone: stored.time_zone,
        resolved_time_zone: resolved,
        system_time_zone: system,
        preferred_name: stored.preferred_name,
    }
}

fn normalize_preferred_name(value: &str) -> Result<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > 80 {
        return Err(anyhow!("preferred name must be 80 characters or fewer"));
    }
    if value.chars().any(char::is_control) {
        return Err(anyhow!("preferred name cannot contain control characters"));
    }
    Ok(Some(value.to_string()))
}

fn persist(dir: &Path, stored: &StoredSettings) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = config_path(dir);
    let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(stored)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Load the preference before any database, capture, or calendar work computes
/// "today". A missing file intentionally follows the Mac's current time zone.
pub fn init(dir: &Path) {
    let stored = std::fs::read_to_string(config_path(dir))
        .ok()
        .and_then(|raw| serde_json::from_str::<StoredSettings>(&raw).ok())
        .filter(|settings| resolve_time_zone(&settings.time_zone).is_ok())
        .unwrap_or_default();
    let (_, zone) = resolve_time_zone(&stored.time_zone).unwrap_or_else(|_| {
        let fallback = parse_time_zone(FALLBACK_TIME_ZONE).unwrap();
        (FALLBACK_TIME_ZONE.to_string(), fallback)
    });
    *settings_cell().write().unwrap() = stored;
    *time_zone_cell().write().unwrap() = zone;
}

pub fn get() -> SystemSettings {
    snapshot()
}

pub fn time_zone() -> chrono_tz::Tz {
    *time_zone_cell().read().unwrap()
}

pub fn update(
    dir: &Path,
    preference: &str,
    preferred_name: Option<&str>,
) -> Result<SystemSettings> {
    let preference = preference.trim();
    let (_, zone) = resolve_time_zone(preference)?;
    let mut stored = settings_cell().read().unwrap().clone();
    stored.time_zone = preference.to_string();
    if let Some(preferred_name) = preferred_name {
        stored.preferred_name = normalize_preferred_name(preferred_name)?;
    }
    persist(dir, &stored)?;
    *settings_cell().write().unwrap() = stored;
    *time_zone_cell().write().unwrap() = zone;
    Ok(snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_iana_name_from_macos_localtime_link() {
        let path = Path::new("/var/db/timezone/zoneinfo/America/Los_Angeles");
        assert_eq!(
            zone_name_from_localtime(path).as_deref(),
            Some("America/Los_Angeles")
        );
    }

    #[test]
    fn rejects_abbreviations_and_accepts_iana_zones() {
        assert!(resolve_time_zone("PST").is_err());
        assert_eq!(
            resolve_time_zone("America/Los_Angeles").unwrap().0,
            "America/Los_Angeles"
        );
    }

    #[test]
    fn preferred_name_is_trimmed_and_optional() {
        assert_eq!(
            normalize_preferred_name("  Chris  ").unwrap().as_deref(),
            Some("Chris")
        );
        assert_eq!(normalize_preferred_name("   ").unwrap(), None);
        assert!(normalize_preferred_name(&"x".repeat(81)).is_err());
        assert!(normalize_preferred_name("Chris\nRollet").is_err());
    }
}
