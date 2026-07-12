import { api, type ChatProposal, type ThemeCandidate } from "./api";
import type { ThemePack } from "./useTheme";

const STYLE_WORDS = /\b(theme|retheme|restyle|appearance|visual style|skin|look and feel)\b/i;
const ACTION_WORDS = /\b(apply|change|give|make|set|switch|use|want|try|preview|retheme|restyle)\b/i;

export function isThemeRequest(text: string): boolean {
  return STYLE_WORDS.test(text) && ACTION_WORDS.test(text);
}

function deterministicChoice(prompt: string, themes: readonly ThemePack[]): ThemePack {
  const q = prompt.toLowerCase();
  const preferred =
    /apple|mac|ios|cupertino/.test(q) ? "cupertino"
      : /linear|violet|precision|graphite/.test(q) ? "linear-midnight"
        : /paper|notion|notebook|journal/.test(q) ? "paper"
          : /editorial|magazine|newspaper|serif/.test(q) ? "editorial"
            : /terminal|hacker|mono|phosphor/.test(q) ? "terminal"
              : /glass|airy|soft|translucent/.test(q) ? "soft-glass"
                : /contrast|accessible|legible/.test(q) ? "high-contrast"
                  : /warm|original|default/.test(q) ? "noted-warm"
                    : "noted-warm";
  return themes.find((theme) => theme.id === preferred) ?? themes[0];
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
