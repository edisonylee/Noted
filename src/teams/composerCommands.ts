export type ComposerAction = "meeting" | "attach";
export const composerActions = [
  {
    id: "attach" as const,
    label: "Attach files",
    description: "Images, PDFs, and text files",
  },
  {
    id: "meeting" as const,
    label: "Reference a meeting",
    description: "Add a link to shared meeting notes",
  },
];

// A leading command only, never a URL, code block, or a slash in ordinary prose.
export function slashCommands(
  value: string,
  caret: number,
  available: ComposerAction[],
) {
  const prefix = value.slice(0, caret);
  if (!/^\/[a-z]*$/i.test(prefix) || (value[caret] && !/\s/.test(value[caret])))
    return [];
  const query = prefix.slice(1).toLowerCase();
  return composerActions.filter(
    (action) =>
      available.includes(action.id) &&
      (action.id.startsWith(query) ||
        action.label.toLowerCase().includes(query)),
  );
}
