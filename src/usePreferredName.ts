import { useSyncExternalStore } from "react";

let preferredName: string | null = null;
const listeners = new Set<() => void>();

export function configurePreferredName(value: string | null | undefined): void {
  const next = value?.trim() || null;
  if (next === preferredName) return;
  preferredName = next;
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function snapshot(): string | null {
  return preferredName;
}

export function usePreferredName(): string | null {
  return useSyncExternalStore(subscribe, snapshot, snapshot);
}
