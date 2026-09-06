import { useEffect, useState, type Dispatch, type SetStateAction } from "react";

// Keep navigation across view unmounts during this app session. Store locations
// and filters, not fetched content, credentials, or modal/form state.
const locations = new Map<string, unknown>();
export function clearNavigationState(prefix: string) {
  for (const key of locations.keys()) if (key.startsWith(prefix)) locations.delete(key);
}
export function useNavigationState<T>(key: string, initial: T): [T, Dispatch<SetStateAction<T>>] {
  const [value, setValue] = useState<T>(() => locations.has(key) ? locations.get(key) as T : initial);
  useEffect(() => { locations.set(key, value); }, [key, value]);
  return [value, setValue];
}
