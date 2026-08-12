import {
  useEffect,
  useId,
  useRef,
  useState,
  type ComponentType,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import {
  ChevronDown,
  Cloud,
  CloudDrizzle,
  CloudFog,
  CloudLightning,
  CloudMoon,
  CloudRain,
  CloudSnow,
  CloudSun,
  MapPin,
  Moon,
  RefreshCw,
  Search,
  Sun,
  X,
} from "lucide-react";
import { api } from "./api";
import { configureAppTimeZone } from "./day";

type WeatherLocation = {
  name: string;
  region: string;
  latitude: number;
  longitude: number;
  timezone: string;
};

// First-run fallback. A city chosen from the weather bar replaces it locally.
const ATLANTA_WEATHER_LOCATION: WeatherLocation = {
  name: "Atlanta",
  region: "Georgia",
  latitude: 33.749,
  longitude: -84.38798,
  timezone: "America/New_York",
};

const LOCATION_KEY = "noted-weather-location-v1";
const GEOCODING_URL = "https://geocoding-api.open-meteo.com/v1/search";

type GeocodingResult = {
  name?: string;
  latitude?: number;
  longitude?: number;
  timezone?: string;
  feature_code?: string;
  admin1?: string;
  country?: string;
};

function validTimezone(value: string): boolean {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone: value }).format();
    return true;
  } catch {
    return false;
  }
}

function isWeatherLocation(value: unknown): value is WeatherLocation {
  if (!value || typeof value !== "object") return false;
  const location = value as Partial<WeatherLocation>;
  return (
    typeof location.name === "string" &&
    location.name.trim().length > 0 &&
    typeof location.region === "string" &&
    typeof location.latitude === "number" &&
    Number.isFinite(location.latitude) &&
    typeof location.longitude === "number" &&
    Number.isFinite(location.longitude) &&
    typeof location.timezone === "string" &&
    validTimezone(location.timezone)
  );
}

function readSavedLocation(): WeatherLocation {
  try {
    const saved = JSON.parse(localStorage.getItem(LOCATION_KEY) ?? "null");
    if (isWeatherLocation(saved)) return saved;
  } catch {
    // A blocked or malformed local preference should never break the homepage.
  }
  return ATLANTA_WEATHER_LOCATION;
}

function saveLocation(location: WeatherLocation) {
  try {
    localStorage.setItem(LOCATION_KEY, JSON.stringify(location));
  } catch {
    // The selection still works for this session when storage is unavailable.
  }
}

async function searchLocations(query: string, signal: AbortSignal): Promise<WeatherLocation[]> {
  const params = new URLSearchParams({
    name: query,
    count: "10",
    language: "en",
    format: "json",
  });
  const response = await fetch(`${GEOCODING_URL}?${params}`, { signal });
  if (!response.ok) throw new Error(`Location search returned ${response.status}`);
  const body = (await response.json()) as { results?: GeocodingResult[] };
  const seen = new Set<string>();

  return (body.results ?? []).flatMap((result) => {
    if (
      typeof result.name !== "string" ||
      typeof result.latitude !== "number" ||
      typeof result.longitude !== "number" ||
      typeof result.timezone !== "string" ||
      (typeof result.feature_code === "string" && !result.feature_code.startsWith("PPL")) ||
      !validTimezone(result.timezone)
    ) {
      return [];
    }
    const regionParts = [result.admin1, result.country].filter(
      (part, index, parts): part is string => !!part && parts.indexOf(part) === index,
    );
    const location: WeatherLocation = {
      name: result.name,
      region: regionParts.join(", "),
      latitude: result.latitude,
      longitude: result.longitude,
      timezone: result.timezone,
    };
    const key = keyFor(location);
    if (seen.has(key)) return [];
    seen.add(key);
    return [location];
  }).slice(0, 6);
}

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
    windDirection: number;
    precipitation: number;
    rain: number;
    showers: number;
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

const CACHE_KEY = "noted-weather-v2";
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
    typeof candidate.current.windDirection === "number" &&
    typeof candidate.current.precipitation === "number" &&
    typeof candidate.current.rain === "number" &&
    typeof candidate.current.showers === "number" &&
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
      "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,cloud_cover,wind_speed_10m,wind_direction_10m,precipitation,rain,showers,is_day",
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
        windDirection: numberValue(body.current.wind_direction_10m, "wind direction"),
        precipitation: numberValue(body.current.precipitation, "current precipitation"),
        rain: numberValue(body.current.rain, "current rain"),
        showers: numberValue(body.current.showers, "current showers"),
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

function weatherCondition(code: number): { label: string; kind: WeatherKind } {
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
    <span className="weather-glyph" aria-hidden="true">
      <Icon size={19} strokeWidth={1.7} />
    </span>
  );
}

function rounded(value: number): string {
  return String(Math.round(value));
}

function weatherDate(date: string): string {
  const [year, month, day] = date.split("-").map(Number);
  return new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    month: "short",
    day: "numeric",
    timeZone: "UTC",
  }).format(new Date(Date.UTC(year, month - 1, day)));
}

type RainKind = Extract<WeatherKind, "drizzle" | "rain" | "storm">;

type RainDrop = {
  x: number;
  delay: number;
  duration: number;
  length: number;
  width: number;
  alpha: number;
  drift: number;
  near: boolean;
};

const RAIN_CONFIG: Record<
  RainKind,
  { count: number; seed: number; duration: [number, number]; length: [number, number]; alpha: [number, number] }
> = {
  drizzle: { count: 26, seed: 5101, duration: [1.7, 2.55], length: [4, 10], alpha: [0.2, 0.42] },
  rain: { count: 40, seed: 6103, duration: [0.9, 1.4], length: [10, 22], alpha: [0.28, 0.56] },
  storm: { count: 50, seed: 9503, duration: [0.58, 0.96], length: [16, 32], alpha: [0.34, 0.66] },
};

function seededUnit(index: number, salt: number): number {
  const value = Math.sin((index + 1) * 12.9898 + salt * 78.233) * 43758.5453;
  return value - Math.floor(value);
}

function makeRainDrops(kind: RainKind): RainDrop[] {
  const config = RAIN_CONFIG[kind];
  return Array.from({ length: config.count }, (_, index) => {
    const near = seededUnit(index, config.seed + 1) > 0.68;
    const duration =
      config.duration[0] +
      seededUnit(index, config.seed + 2) * (config.duration[1] - config.duration[0]);
    const depthScale = near ? 1.18 : 0.78;
    return {
      x: -4 + seededUnit(index, config.seed + 3) * 108,
      delay: -duration * seededUnit(index, config.seed + 4),
      duration,
      length:
        (config.length[0] +
          seededUnit(index, config.seed + 5) * (config.length[1] - config.length[0])) *
        depthScale,
      width: (0.65 + seededUnit(index, config.seed + 6) * 0.7) * depthScale,
      alpha:
        (config.alpha[0] +
          seededUnit(index, config.seed + 7) * (config.alpha[1] - config.alpha[0])) *
        (near ? 1 : 0.68),
      drift: -8 + seededUnit(index, config.seed + 8) * 16,
      near,
    };
  });
}

const RAIN_DROPS: Record<RainKind, RainDrop[]> = {
  drizzle: makeRainDrops("drizzle"),
  rain: makeRainDrops("rain"),
  storm: makeRainDrops("storm"),
};

function RainParticles({ kind }: { kind: RainKind }) {
  return (
    <span className={`weather-rain weather-rain-${kind}`}>
      {RAIN_DROPS[kind].map((drop, index) => (
        <i
          key={index}
          className={`weather-rain-drop${drop.near ? " weather-rain-drop-near" : ""}`}
          style={
            {
              "--rain-x": `${drop.x}%`,
              "--rain-delay": `${drop.delay}s`,
              "--rain-duration": `${drop.duration}s`,
              "--rain-length": `${drop.length}px`,
              "--rain-width": `${drop.width}px`,
              "--rain-alpha": String(drop.alpha),
              "--rain-drop-drift": `${drop.drift}px`,
            } as CSSProperties
          }
        />
      ))}
    </span>
  );
}

function WeatherAtmosphere({
  kind,
  cloudCover,
  windSpeed,
  windDirection,
  precipitation,
}: {
  kind: WeatherKind | "loading";
  cloudCover: number;
  windSpeed: number;
  windDirection: number;
  precipitation: number;
}) {
  const density = Math.min(1, Math.max(0, cloudCover / 100));
  const driftSeconds = Math.min(52, Math.max(18, 48 - windSpeed * 1.25));
  const rainKind: RainKind | null =
    kind === "drizzle" || kind === "rain" || kind === "storm" ? kind : null;
  const rainIntensity = Math.min(1, Math.max(0, precipitation / 0.12));
  const windAcrossScreen = -Math.sin((windDirection * Math.PI) / 180);
  const rainDrift = windAcrossScreen * Math.min(16, 2 + windSpeed * 0.55);
  const rainOpacity =
    kind === "drizzle"
      ? 0.32 + rainIntensity * 0.16
      : kind === "rain"
        ? 0.44 + rainIntensity * 0.18
        : 0.54 + rainIntensity * 0.18;
  const sceneStyle = {
    "--weather-cloud-far-opacity": String(0.1 + density * 0.34),
    "--weather-cloud-mid-opacity": String(0.14 + density * 0.48),
    "--weather-cloud-near-opacity": String(0.08 + density * 0.3),
    "--weather-drift-duration": `${driftSeconds}s`,
    "--weather-rain-opacity": String(rainOpacity),
    "--weather-rain-drift": `${rainDrift.toFixed(1)}vw`,
    "--weather-rain-angle": `${(-rainDrift * 0.7).toFixed(1)}deg`,
  } as CSSProperties;

  return (
    <div className="weather-atmosphere" style={sceneStyle} aria-hidden="true">
      <span className="weather-orb" />
      <span className="weather-cloud weather-cloud-one" />
      <span className="weather-cloud weather-cloud-two" />
      <span className="weather-cloud weather-cloud-three" />
      {rainKind && <RainParticles kind={rainKind} />}
      {kind === "snow" && (
        <>
          <span className="weather-precip weather-precip-far" />
          <span className="weather-precip weather-precip-near" />
        </>
      )}
      <span className="weather-haze" />
      <span className="weather-storm-light" />
    </div>
  );
}

function WeatherLocationPicker({
  location,
  align,
  onChange,
}: {
  location: WeatherLocation;
  align: "start" | "end";
  onChange: (location: WeatherLocation) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<WeatherLocation[]>([]);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [updateError, setUpdateError] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listId = useId();

  useEffect(() => {
    if (!open) return;
    const frame = requestAnimationFrame(() => inputRef.current?.focus());
    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        requestAnimationFrame(() => rootRef.current?.querySelector<HTMLButtonElement>("button")?.focus());
      }
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("pointerdown", closeOnOutsideClick);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const trimmed = query.trim();
    if (trimmed.length < 2) {
      setResults([]);
      setSearching(false);
      setSearchError(false);
      setActiveIndex(0);
      return;
    }

    const controller = new AbortController();
    setResults([]);
    setSearching(true);
    setSearchError(false);
    const timeout = window.setTimeout(async () => {
      try {
        const next = await searchLocations(trimmed, controller.signal);
        setResults(next);
        setActiveIndex(0);
      } catch (error) {
        if ((error as Error).name !== "AbortError") {
          setResults([]);
          setSearchError(true);
        }
      } finally {
        if (!controller.signal.aborted) setSearching(false);
      }
    }, 250);

    return () => {
      window.clearTimeout(timeout);
      controller.abort();
    };
  }, [open, query]);

  function close() {
    setOpen(false);
    setQuery("");
    setResults([]);
    setSearchError(false);
    requestAnimationFrame(() => rootRef.current?.querySelector<HTMLButtonElement>("button")?.focus());
  }

  async function choose(next: WeatherLocation) {
    if (updating) return;
    setUpdating(true);
    setUpdateError(false);
    try {
      await onChange(next);
      close();
    } catch {
      setUpdateError(true);
    } finally {
      setUpdating(false);
    }
  }

  function onSearchKeyDown(event: ReactKeyboardEvent<HTMLInputElement>) {
    if (updating || !results.length) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((index) => (index + 1) % results.length);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((index) => (index - 1 + results.length) % results.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      void choose(results[activeIndex]);
    }
  }

  return (
    <div className={`weather-location-control weather-location-${align}`} ref={rootRef}>
      <button
        className="weather-bar-place weather-location-trigger"
        type="button"
        onClick={() => setOpen((current) => !current)}
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Change weather city"
      >
        <MapPin size={13} />
        <span>{location.name}</span>
        <ChevronDown size={12} className={open ? "open" : ""} />
      </button>
      {open && (
        <div className="weather-location-popover" role="dialog" aria-label="Change weather city">
          <div className="weather-location-search">
            <Search size={15} aria-hidden="true" />
            <input
              ref={inputRef}
              value={query}
              onChange={(event) => {
                setQuery(event.target.value);
                setUpdateError(false);
              }}
              onKeyDown={onSearchKeyDown}
              placeholder="Search city or postal code"
              aria-label="Search city or postal code"
              role="combobox"
              aria-autocomplete="list"
              aria-expanded="true"
              aria-controls={listId}
              aria-activedescendant={results.length ? `${listId}-${activeIndex}` : undefined}
              autoComplete="off"
              spellCheck={false}
              disabled={updating}
            />
            <button type="button" onClick={close} aria-label="Close city search" disabled={updating}>
              <X size={14} />
            </button>
          </div>
          {(updating || updateError) && (
            <p className={`weather-location-update${updateError ? " error" : ""}`} role="status">
              {updating
                ? "Updating city and time zone…"
                : "Couldn’t update the city and time zone. Try again."}
            </p>
          )}
          <div
            className="weather-location-results"
            id={listId}
            role={results.length ? "listbox" : "status"}
            aria-live="polite"
          >
            {query.trim().length < 2 ? (
              <p>Type at least two characters.</p>
            ) : searching ? (
              <p>Finding places…</p>
            ) : searchError ? (
              <p>City search is unavailable. Try again.</p>
            ) : results.length === 0 ? (
              <p>No matching cities.</p>
            ) : (
              results.map((result, index) => (
                <button
                  key={keyFor(result)}
                  id={`${listId}-${index}`}
                  type="button"
                  role="option"
                  aria-selected={index === activeIndex}
                  className={index === activeIndex ? "active" : ""}
                  onMouseEnter={() => setActiveIndex(index)}
                  onFocus={() => setActiveIndex(index)}
                  onClick={() => void choose(result)}
                  disabled={updating}
                >
                  <strong>{result.name}</strong>
                  <span>{result.region || result.timezone}</span>
                </button>
              ))
            )}
          </div>
          <a
            className="weather-location-credit"
            href="https://open-meteo.com/en/docs/geocoding-api"
            target="_blank"
            rel="noreferrer"
          >
            Locations by Open-Meteo
          </a>
        </div>
      )}
    </div>
  );
}

export function WeatherHome({
  location: initialLocation,
  onTimeZoneChange,
  children,
}: {
  location?: WeatherLocation;
  onTimeZoneChange?: (timeZone: string) => void;
  children: ReactNode;
}) {
  const [location, setLocation] = useState<WeatherLocation>(() => initialLocation ?? readSavedLocation());
  const [snapshot, setSnapshot] = useState<WeatherSnapshot | null>(() => readCache(location));
  const [loading, setLoading] = useState(() => !readCache(location));
  const [unavailable, setUnavailable] = useState(false);
  const locationKey = keyFor(location);

  async function changeLocation(next: WeatherLocation) {
    const settings = await api.systemSettingsSet(next.timezone);
    configureAppTimeZone(settings.resolvedTimeZone);
    saveLocation(next);
    const cached = readCache(next);
    setSnapshot(cached);
    setLoading(!cached);
    setUnavailable(false);
    setLocation(next);
    onTimeZoneChange?.(settings.resolvedTimeZone);
  }

  async function load(force = false) {
    setLoading(true);
    setUnavailable(false);
    try {
      setSnapshot(await getForecast(location, force));
    } catch {
      setUnavailable(true);
    } finally {
      setLoading(false);
    }
  }

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
  }, [locationKey]);

  if (!snapshot) {
    return (
      <section className="weather-home weather-home-loading">
        <WeatherAtmosphere
          kind="loading"
          cloudCover={55}
          windSpeed={5}
          windDirection={0}
          precipitation={0}
        />
        <header className="weather-bar" aria-live="polite">
          <WeatherLocationPicker location={location} align="start" onChange={changeLocation} />
          <span className="weather-bar-status">
            <Cloud size={18} /> {loading ? "Checking weather…" : "Weather unavailable"}
          </span>
          <span className="weather-bar-spacer" />
          <span className="weather-bar-date">{weatherDate(localDate(location.timezone))}</span>
          {!loading && (
            <button className="weather-bar-refresh" onClick={() => load(true)} aria-label="Retry weather">
              <RefreshCw size={14} />
            </button>
          )}
        </header>
        <div className="weather-home-content">{children}</div>
      </section>
    );
  }

  const condition = weatherCondition(snapshot.current.weatherCode);
  const stale = unavailable || Date.now() - snapshot.fetchedAt >= FRESH_FOR_MS;
  const time = snapshot.current.isDay ? "day" : "night";

  return (
    <section className={`weather-home weather-home-${condition.kind} weather-home-${time}`}>
      <WeatherAtmosphere
        kind={condition.kind}
        cloudCover={snapshot.current.cloudCover}
        windSpeed={snapshot.current.windSpeed}
        windDirection={snapshot.current.windDirection}
        precipitation={Math.max(
          snapshot.current.precipitation,
          snapshot.current.rain + snapshot.current.showers,
        )}
      />
      <header
        className="weather-bar"
        aria-label={`${weatherDate(snapshot.today.date)}. ${location.name} weather: ${rounded(snapshot.current.temperature)} degrees, ${condition.label}. High ${rounded(snapshot.today.high)}, low ${rounded(snapshot.today.low)}.`}
      >
        <span className="weather-bar-current">
          <WeatherIcon kind={condition.kind} isDay={snapshot.current.isDay} />
          <strong>{rounded(snapshot.current.temperature)}°</strong>
          <span>{condition.label}</span>
          {stale && <em>saved</em>}
        </span>
        <span className="weather-bar-date">{weatherDate(snapshot.today.date)}</span>
        <span className="weather-bar-spacer" />
        <span className="weather-bar-range">
          <b>H</b> {rounded(snapshot.today.high)}° <b>L</b> {rounded(snapshot.today.low)}°
        </span>
        <WeatherLocationPicker location={location} align="end" onChange={changeLocation} />
        <a href="https://open-meteo.com/" target="_blank" rel="noreferrer">
          Weather by Open-Meteo
        </a>
        <button
          className="weather-bar-refresh"
          onClick={() => load(true)}
          disabled={loading}
          aria-label="Refresh weather"
          title="Refresh weather"
        >
          <RefreshCw size={13} className={loading ? "spin" : ""} />
        </button>
      </header>
      <div className="weather-home-content">{children}</div>
    </section>
  );
}
