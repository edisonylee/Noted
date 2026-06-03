import { listen as tauriListen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { isDesktop } from "./api";

// Tauri's event bus only exists in the desktop shell. In the phone browser there
// is no backend pushing events (photo-received / recap-generated), so listen()
// becomes a no-op — the affected views just refresh on navigation instead.
export function listen<T>(event: string, cb: EventCallback<T>): Promise<UnlistenFn> {
  if (isDesktop) return tauriListen<T>(event, cb);
  return Promise.resolve(() => {});
}
