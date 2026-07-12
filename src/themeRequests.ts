import { api, type ChatProposal, type ThemeCandidate } from "./api";
import type { ThemePack } from "./useTheme";

const STYLE_WORDS = /\b(theme|retheme|restyle|appearance|visual style|skin|look and feel)\b/i;
const ACTION_WORDS = /\b(apply|change|give|make|set|switch|use|want|try|preview|retheme|restyle)\b/i;

export function isThemeRequest(text: string): boolean {
  return STYLE_WORDS.test(text) && ACTION_WORDS.test(text);
}

function deterministicChoice(prompt: string, themes: readonly ThemePack[]): ThemePack {
  const q = prompt.toLowerCase();
  const exact = themes.find((theme) =>
    q.includes(theme.name.toLowerCase()) || q.includes(theme.id.replace(/-/g, " "))
  );
  if (exact) return exact;

  const preferred = /apple|mac|ios/.test(q) ? "cupertino"
    : /contrast|accessible|legible/.test(q) ? "high-contrast"
      : /warm|original|default/.test(q) ? "noted-warm"
        : null;
  if (preferred) return themes.find((theme) => theme.id === preferred) ?? themes[0];

  const ignored = new Set(["theme", "retheme", "restyle", "appearance", "visual", "style", "look", "feel", "make", "give", "want", "apply", "change", "switch", "preview", "noted", "with", "like", "more", "very"]);
  const words = q.match(/[a-z]{3,}/g)?.filter((word) => !ignored.has(word)) ?? [];
  let best = themes[0];
  let bestScore = 0;
  for (const theme of themes) {
    const name = theme.name.toLowerCase();
    const description = theme.description?.toLowerCase() ?? "";
    const score = words.reduce((total, word) =>
      total + (name.includes(word) ? 3 : 0) + (description.includes(word) ? 1 : 0), 0);
    if (score > bestScore) {
      best = theme;
      bestScore = score;
    }
  }
  return bestScore > 0 ? best : themes.find((theme) => theme.id === "noted-warm") ?? themes[0];
}

export async function proposeTheme(prompt: string, themes: readonly ThemePack[]): Promise<ChatProposal> {
  if (!themes.length) throw new Error("No themes are installed.");
  const candidates: ThemeCandidate[] = themes.map((theme) => ({
    id: theme.id,
    name: theme.name,
    description: theme.description ?? "",
  }));
  let chosen = deterministicChoice(prompt, themes);
  let summary = `“${chosen.name}” is the closest installed match for that visual direction.`;

  try {
    const suggestion = await api.themeSuggest(prompt, candidates);
    const match = themes.find((theme) => theme.id === suggestion.themeId);
    if (match) {
      chosen = match;
      summary = suggestion.summary;
    }
  } catch {
    // Keyword matching keeps common requests available when Ollama is offline.
  }

  return {
    action: "apply_theme",
    theme_id: chosen.id,
    theme_name: chosen.name,
    summary,
  };
}
