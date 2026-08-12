export type FilingContext = "work" | "personal";

const STORAGE_KEY = "noted-filing-context";
const CHANGE_EVENT = "noted-filing-context-change";

export function isFilingContext(value: unknown): value is FilingContext {
  return value === "work" || value === "personal";
}

export function readFilingContext(): FilingContext {
  const saved = localStorage.getItem(STORAGE_KEY);
  return isFilingContext(saved) ? saved : "work";
}

export function hasStoredFilingContext(): boolean {
  return isFilingContext(localStorage.getItem(STORAGE_KEY));
}

export function writeFilingContext(context: FilingContext): void {
  localStorage.setItem(STORAGE_KEY, context);
  window.dispatchEvent(new CustomEvent<FilingContext>(CHANGE_EVENT, { detail: context }));
}

export function onFilingContextChange(listener: (context: FilingContext) => void): () => void {
  const onChange = (event: Event) => {
    const context = (event as CustomEvent<unknown>).detail;
    if (isFilingContext(context)) listener(context);
  };
  window.addEventListener(CHANGE_EVENT, onChange);
  return () => window.removeEventListener(CHANGE_EVENT, onChange);
}

export function filingContextLabel(context: FilingContext): string {
  return context === "personal" ? "Personal" : "Work";
}
