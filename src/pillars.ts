// The three life pillars `noted` is organized around. "Other" catches anything
// the heuristic can't confidently place (categories are emergent, so this maps
// names → pillars by sensible defaults first, then a keyword fallback).
export type Pillar = "Physical" | "Mental" | "Social" | "Other";

export const PILLAR_ORDER: Pillar[] = ["Physical", "Mental", "Social", "Other"];

// Exact category-name overrides (checked first).
const DEFAULTS: Record<string, Pillar> = {
  gym: "Physical",
  workout: "Physical",
  workouts: "Physical",
  lifting: "Physical",
  cardio: "Physical",
  running: "Physical",
  diet: "Physical",
  food: "Physical",
  meals: "Physical",
  nutrition: "Physical",
  sleep: "Physical",
  injuries: "Physical",
  injury: "Physical",
  health: "Physical",
  mood: "Mental",
  emotions: "Mental",
  mental: "Mental",
  people: "Social",
  social: "Social",
  relationships: "Social",
};

// Keyword fallback over name + description, in priority order.
const KEYWORDS: [RegExp, Pillar][] = [
  [/\b(gym|workout|lift|squat|bench|deadlift|run|cardio|diet|food|meal|eat|sleep|injur|stretch|body|weight|nutri|health|rehab)\b/i, "Physical"],
  [/\b(mood|feel|feeling|emotion|anxi|stress|mental|happy|sad|grateful|reflect)\b/i, "Mental"],
  [/\b(people|person|friend|family|social|relationship|coworker|colleague|client)\b/i, "Social"],
];

/** Classify a category into a life pillar. */
export function pillarOf(name: string, description = ""): Pillar {
  const c = name.trim().toLowerCase();
  if (DEFAULTS[c]) return DEFAULTS[c];
  const hay = `${c} ${description}`.toLowerCase();
  for (const [re, p] of KEYWORDS) if (re.test(hay)) return p;
  return "Other";
}

/** Bucket categories by pillar, in canonical order, dropping empty pillars. */
export function groupByPillar<T extends { name: string; description?: string }>(
  cats: T[]
): { pillar: Pillar; cats: T[] }[] {
  const buckets = new Map<Pillar, T[]>();
  for (const cat of cats) {
    const p = pillarOf(cat.name, cat.description ?? "");
    if (!buckets.has(p)) buckets.set(p, []);
    buckets.get(p)!.push(cat);
  }
  return PILLAR_ORDER.filter((p) => buckets.has(p)).map((pillar) => ({
    pillar,
    cats: buckets.get(pillar)!,
  }));
}
