import { useCallback, useEffect, useState, type ComponentType } from "react";
import {
  Cloud,
  CloudDrizzle,
  CloudFog,
  CloudLightning,
  CloudMoon,
  CloudRain,
  CloudSnow,
  CloudSun,
  Droplets,
  MapPin,
  Moon,
  RefreshCw,
  Sun,
  Umbrella,
  Wind,
} from "lucide-react";

export type WeatherLocation = {
  name: string;
  region: string;
  latitude: number;
  longitude: number;
  timezone: string;
};

// Kept as a single value so Settings can replace it later without changing the
// card, request, or cache shape.
export const ATLANTA_WEATHER_LOCATION: WeatherLocation = {
  name: "Atlanta",
  region: "Georgia",
  latitude: 33.749,
  longitude: -84.38798,
  timezone: "America/New_York",
};

type WeatherKind = "clear" | "cloudy" | "fog" | "drizzle" | "rain" | "snow" | "storm";

type WeatherSnapshot = {
  fetchedAt: number;
  current: {
    temperature: number;
    apparentTemperature: number;
    humidity: number;
    weatherCode: number;
    cloudCover: number;
    windSpeed: number;
    isDay: boolean;
  };
  today: {
    date: string;
    weatherCode: number;
    high: number;
    low: number;
    precipitationChance: number;
  };
};

type CacheEntry = {
  locationKey: string;
  snapshot: WeatherSnapshot;
};

const CACHE_KEY = "noted-weather-v1";
const FRESH_FOR_MS = 15 * 60_000;
const MAX_CACHE_AGE_MS = 12 * 60 * 60_000;
const REQUEST_TIMEOUT_MS = 10_000;

const memoryCache = new Map<string, WeatherSnapshot>();
const requests = new Map<string, Promise<WeatherSnapshot>>();

function keyFor(location: WeatherLocation): string {
  return `${location.latitude},${location.longitude},${location.timezone}`;
}

function localDate(timezone: string): string {
  const parts = new Intl.DateTimeFormat("en-US", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    timeZone: timezone,
  }).formatToParts();
  const value = (type: "year" | "month" | "day") =>
    parts.find((part) => part.type === type)?.value ?? "";
  return `${value("year")}-${value("month")}-${value("day")}`;
}

function isSnapshot(value: unknown): value is WeatherSnapshot {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<WeatherSnapshot>;
  return (
    typeof candidate.fetchedAt === "number" &&
    !!candidate.current &&
    typeof candidate.current.temperature === "number" &&
    typeof candidate.current.weatherCode === "number" &&
    !!candidate.today &&
    typeof candidate.today.date === "string" &&
    typeof candidate.today.high === "number" &&
    typeof candidate.today.low === "number"
  );
}

function readCache(location: WeatherLocation): WeatherSnapshot | null {
  const key = keyFor(location);
  const inMemory = memoryCache.get(key);
  if (
    inMemory &&
    inMemory.today.date === localDate(location.timezone) &&
    Date.now() - inMemory.fetchedAt <= MAX_CACHE_AGE_MS
  ) {
    return inMemory;
  }

  try {
    const raw = localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const cached = JSON.parse(raw) as Partial<CacheEntry>;
    if (
      cached.locationKey !== key ||
      !isSnapshot(cached.snapshot) ||
      cached.snapshot.today.date !== localDate(location.timezone) ||
      Date.now() - cached.snapshot.fetchedAt > MAX_CACHE_AGE_MS
    ) {
      return null;
    }
    memoryCache.set(key, cached.snapshot);
    return cached.snapshot;
  } catch {
    return null;
  }
}

function writeCache(location: WeatherLocation, snapshot: WeatherSnapshot) {
  const locationKey = keyFor(location);
  memoryCache.set(locationKey, snapshot);
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify({ locationKey, snapshot } satisfies CacheEntry));
  } catch {
    // Weather still works when storage is unavailable (private browsing, quota, etc.).
  }
}

function buildForecastUrl(location: WeatherLocation): string {
  const params = new URLSearchParams({
    latitude: String(location.latitude),
    longitude: String(location.longitude),
    current:
      "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,cloud_cover,wind_speed_10m,is_day",
    daily: "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max",
    temperature_unit: "fahrenheit",
    wind_speed_unit: "mph",
    precipitation_unit: "inch",
    timezone: location.timezone,
    forecast_days: "1",
  });
  return `https://api.open-meteo.com/v1/forecast?${params}`;
}

function numberValue(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`Weather response is missing ${field}`);
  }
  return value;
}

function firstNumber(value: unknown, field: string): number {
  if (!Array.isArray(value)) throw new Error(`Weather response is missing ${field}`);
  return numberValue(value[0], field);
}

function firstString(value: unknown, field: string): string {
  if (!Array.isArray(value) || typeof value[0] !== "string") {
    throw new Error(`Weather response is missing ${field}`);
  }
  return value[0];
}

async function fetchForecast(location: WeatherLocation): Promise<WeatherSnapshot> {
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), REQUEST_TIMEOUT_MS);
  try {
    const response = await fetch(buildForecastUrl(location), { signal: controller.signal });
    if (!response.ok) throw new Error(`Weather service returned ${response.status}`);

    const body = (await response.json()) as {
      current?: Record<string, unknown>;
      daily?: Record<string, unknown>;
    };
    if (!body.current || !body.daily) throw new Error("Weather response is incomplete");

    const snapshot: WeatherSnapshot = {
      fetchedAt: Date.now(),
      current: {
        temperature: numberValue(body.current.temperature_2m, "current temperature"),
        apparentTemperature: numberValue(
          body.current.apparent_temperature,
          "apparent temperature",
        ),
        humidity: numberValue(body.current.relative_humidity_2m, "humidity"),
        weatherCode: numberValue(body.current.weather_code, "current weather code"),
        cloudCover: numberValue(body.current.cloud_cover, "cloud cover"),
        windSpeed: numberValue(body.current.wind_speed_10m, "wind speed"),
        isDay: numberValue(body.current.is_day, "daylight status") === 1,
      },
      today: {
        date: firstString(body.daily.time, "today's date"),
        weatherCode: firstNumber(body.daily.weather_code, "today's weather code"),
        high: firstNumber(body.daily.temperature_2m_max, "today's high"),
        low: firstNumber(body.daily.temperature_2m_min, "today's low"),
        precipitationChance: firstNumber(
          body.daily.precipitation_probability_max,
          "today's precipitation chance",
        ),
      },
    };
    writeCache(location, snapshot);
    return snapshot;
  } finally {
    window.clearTimeout(timeout);
  }
}

async function getForecast(location: WeatherLocation, force = false): Promise<WeatherSnapshot> {
  const key = keyFor(location);
  const cached = readCache(location);
  if (!force && cached && Date.now() - cached.fetchedAt < FRESH_FOR_MS) return cached;

  const active = requests.get(key);
  if (active) return active;

  const request = fetchForecast(location).finally(() => requests.delete(key));
  requests.set(key, request);
  return request;
}

export function weatherCondition(code: number): { label: string; kind: WeatherKind } {
  if (code === 0) return { label: "Clear", kind: "clear" };
  if (code === 1) return { label: "Mostly clear", kind: "clear" };
  if (code === 2) return { label: "Partly cloudy", kind: "cloudy" };
  if (code === 3) return { label: "Overcast", kind: "cloudy" };
  if (code === 45 || code === 48) return { label: "Foggy", kind: "fog" };
  if (code >= 51 && code <= 57) return { label: "Drizzle", kind: "drizzle" };
  if ((code >= 61 && code <= 67) || (code >= 80 && code <= 82)) {
    return { label: code >= 80 ? "Rain showers" : "Rain", kind: "rain" };
  }
  if ((code >= 71 && code <= 77) || (code >= 85 && code <= 86)) {
    return { label: code >= 85 ? "Snow showers" : "Snow", kind: "snow" };
  }
  if (code >= 95 && code <= 99) return { label: "Thunderstorms", kind: "storm" };
  return { label: "Mixed conditions", kind: "cloudy" };
}

function WeatherIcon({ kind, isDay }: { kind: WeatherKind; isDay: boolean }) {
  let Icon: ComponentType<{ size?: number; strokeWidth?: number }>;
  if (kind === "clear") Icon = isDay ? Sun : Moon;
  else if (kind === "cloudy") Icon = isDay ? CloudSun : CloudMoon;
  else if (kind === "fog") Icon = CloudFog;
  else if (kind === "drizzle") Icon = CloudDrizzle;
  else if (kind === "rain") Icon = CloudRain;
  else if (kind === "snow") Icon = CloudSnow;
  else Icon = CloudLightning;

  return (
    <div className={`weather-art weather-art-${kind}`} aria-hidden="true">
      <Icon size={84} strokeWidth={1.15} />
      {(kind === "cloudy" || kind === "fog") && (
        <Cloud className="weather-art-cloud" size={31} strokeWidth={1.15} />
      )}
    </div>
  );
}

function rounded(value: number): string {
  return String(Math.round(value));
}

function updatedAt(snapshot: WeatherSnapshot, location: WeatherLocation): string {
  return new Date(snapshot.fetchedAt).toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
    timeZone: location.timezone,
  });
}

export function WeatherCard({
  location = ATLANTA_WEATHER_LOCATION,
}: {
  location?: WeatherLocation;
}) {
  const [snapshot, setSnapshot] = useState<WeatherSnapshot | null>(() => readCache(location));
  const [loading, setLoading] = useState(() => !readCache(location));
  const [unavailable, setUnavailable] = useState(false);

  const load = useCallback(
    async (force = false) => {
      setLoading(true);
      setUnavailable(false);
      try {
        setSnapshot(await getForecast(location, force));
      } catch {
        setUnavailable(true);
      } finally {
        setLoading(false);
      }
    },
    [location.latitude, location.longitude, location.timezone],
  );

  useEffect(() => {
    let active = true;
    const safelyLoad = async () => {
      try {
        const next = await getForecast(location);
        if (active) {
          setSnapshot(next);
          setUnavailable(false);
        }
      } catch {
        if (active) setUnavailable(true);
      } finally {
        if (active) setLoading(false);
      }
    };

    safelyLoad();
    const interval = window.setInterval(safelyLoad, FRESH_FOR_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") safelyLoad();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      active = false;
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [location.latitude, location.longitude, location.timezone]);

  if (!snapshot) {
    return (
      <section className="weather-card weather-card-status" aria-live="polite">
        <div className="weather-status-icon" aria-hidden="true">
          <Cloud size={32} strokeWidth={1.25} />
        </div>
        <div>
          <div className="weather-location">
            <MapPin size={13} /> {location.name}, {location.region}
          </div>
          <p className="weather-status-title">
            {loading ? "Checking the weather…" : "Weather is unavailable"}
          </p>
          {!loading && <p className="weather-status-copy">Connect to the internet and try again.</p>}
        </div>
        {!loading && (
          <button className="weather-retry" onClick={() => load(true)}>
            <RefreshCw size={14} /> Retry
          </button>
        )}
      </section>
    );
  }

  const condition = weatherCondition(snapshot.current.weatherCode);
  const outlook = weatherCondition(snapshot.today.weatherCode);
  const stale = unavailable || Date.now() - snapshot.fetchedAt >= FRESH_FOR_MS;

  return (
    <section className={`weather-card weather-card-${condition.kind}`} aria-label={`${location.name} weather`}>
      <div className="weather-main">
        <div className="weather-reading">
          <div className="weather-location">
            <MapPin size={13} /> {location.name}, {location.region}
          </div>
          <div className="weather-temperature">
            {rounded(snapshot.current.temperature)}°
          </div>
          <div className="weather-condition">{condition.label}</div>
          <div className="weather-feels">
            Feels like {rounded(snapshot.current.apparentTemperature)}° · {rounded(snapshot.current.cloudCover)}% cloud cover
          </div>
        </div>
        <WeatherIcon kind={condition.kind} isDay={snapshot.current.isDay} />
      </div>

      <div className="weather-outlook">
        <div className="weather-outlook-head">
          <span>Today’s outlook</span>
          <strong>{outlook.label}</strong>
        </div>
        <div className="weather-details">
          <span><b>H</b> {rounded(snapshot.today.high)}°</span>
          <span><b>L</b> {rounded(snapshot.today.low)}°</span>
          <span><Umbrella size={13} /> {rounded(snapshot.today.precipitationChance)}%</span>
          <span><Droplets size={13} /> {rounded(snapshot.current.humidity)}%</span>
          <span><Wind size={13} /> {rounded(snapshot.current.windSpeed)} mph</span>
        </div>
      </div>

      <div className="weather-source">
        <span>{stale ? "Saved forecast" : `Updated ${updatedAt(snapshot, location)}`}</span>
        <span aria-hidden="true">·</span>
        <a href="https://open-meteo.com/" target="_blank" rel="noreferrer">
          Weather data by Open-Meteo
        </a>
        <button
          className="weather-refresh"
          onClick={() => load(true)}
          disabled={loading}
          aria-label="Refresh weather"
          title="Refresh weather"
        >
          <RefreshCw size={12} className={loading ? "spin" : ""} />
        </button>
      </div>
    </section>
  );
}
