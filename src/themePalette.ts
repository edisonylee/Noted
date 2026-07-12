import type { CSSProperties } from "react";
import type { Theme } from "./useTheme";

function cssVariable(name: string, fallback: string): string {
  if (typeof document === "undefined") return fallback;
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

export function getChartTheme(mode: Theme) {
  const dark = mode === "dark";
  return {
    colors: Array.from({ length: 8 }, (_, index) =>
      cssVariable(`--chart-${index + 1}`, dark ? "#a8a08f" : "#3d79bd")
    ),
    axis: { stroke: cssVariable("--faint", dark ? "#a8a08f" : "#8c857a"), fontSize: 11 },
    grid: cssVariable("--line", dark ? "#322e26" : "#eee9e0"),
    bar: cssVariable("--accent", dark ? "#5797df" : "#3d79bd"),
    tooltip: {
      background: cssVariable("--surface", dark ? "#211e18" : "#ffffff"),
      border: `1px solid ${cssVariable("--line-strong", dark ? "#423c31" : "#e9e5dd")}`,
      borderRadius: cssVariable("--r-md", "12px"),
      color: cssVariable("--ink", dark ? "#f3efe7" : "#1b1916"),
    } satisfies CSSProperties,
  };
}
