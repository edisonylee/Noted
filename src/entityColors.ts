// Muted, warm-ish palette per entity type (harmonizes with the app theme).
// Shared by the Self graph and the per-entity page so a type's color is
// consistent everywhere.
export const TYPE_COLORS: Record<string, string> = {
  person: "#3d79bd",
  place: "#3f7d5b",
  food: "#c2710c",
  activity: "#8a5a4a",
  item: "#7a6c84",
  org: "#5e7e86",
  topic: "#9b8b6e",
  // Brain-vault artifact types (Work lens). Cooler hues to read as "work".
  project: "#5566a8",   // muted indigo
  decision: "#b8902f",  // gold
  reference: "#6b7280", // slate
  doc: "#9aa0a6",       // grey
};

export const colorForType = (t: string) => TYPE_COLORS[t] ?? "#8c857a";
