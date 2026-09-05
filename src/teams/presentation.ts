import type { TeamSpace } from "./types";

// Give the original starter collection a concrete label without renaming user data.
export function collectionName(space: Pick<TeamSpace, "name" | "description">) {
  return space.name === "Team knowledge" &&
    space.description ===
      "Meetings and decisions shared with everyone in this workspace."
    ? "General meetings"
    : space.name;
}

export function collectionAudience(space: Pick<TeamSpace, "visibility">) {
  return space.visibility === "team"
    ? "All team members"
    : "Selected members and admins";
}

export function initials(name: string) {
  return name
    .trim()
    .split(/\s+/)
    .slice(0, 2)
    .map((part) => part[0])
    .join("");
}
