// Muted, warm-ish palette per entity type (harmonizes with the app theme).
// Shared by the Self graph and the per-entity page so a type's color is
// consistent everywhere.
export const TYPE_COLORS: Record<string, string> = {
  person: "var(--chart-1)",
  place: "var(--chart-3)",
  food: "var(--chart-4)",
  activity: "var(--chart-7)",
  item: "var(--chart-8)",
  org: "var(--chart-5)",
  topic: "var(--chart-6)",
  // Brain-vault artifact types (Work lens). Cooler hues to read as "work".
  project: "var(--chart-2)",
  decision: "var(--chart-4)",
  reference: "var(--chart-6)",
  doc: "var(--faint)",
};

export const colorForType = (t: string) => {
  const color = TYPE_COLORS[t] ?? "var(--faint)";
  if (typeof document === "undefined" || !color.startsWith("var(")) return color;
  const variable = color.slice(4, -1);
  return getComputedStyle(document.documentElement).getPropertyValue(variable).trim() || "#8c857a";
};
