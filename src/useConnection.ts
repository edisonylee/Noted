import { useEffect, useRef, useState } from "react";
import { api, isDesktop } from "./api";

// Watches the connection to the Mac when running as the phone web client. The
// desktop app talks over Tauri IPC and is never "offline", so this is a no-op
// there. On the phone it polls `health` (reusing the existing RPC): a slow
// heartbeat while connected so a rebuild that starts mid-idle is still noticed,
// and a fast retry while offline so reconnects feel instant. `markOffline()`
// lets call sites flip us into offline mode the moment a request fails, instead
// of waiting for the next heartbeat.
export function useConnection({ onReconnect }: { onReconnect?: () => void } = {}): {
  online: boolean;
  markOffline: () => void;
} {
  const [online, setOnline] = useState(true);
  const onlineRef = useRef(true);
  onlineRef.current = online;
  const reconnect = useRef(onReconnect);
  reconnect.current = onReconnect;

  const markOffline = () => {
    if (onlineRef.current) setOnline(false);
  };

  useEffect(() => {
    if (isDesktop) return; // desktop uses IPC — never offline
    let alive = true;
    let timer: ReturnType<typeof setTimeout>;

    const tick = async () => {
      let ok = false;
      try {
        await api.health();
        ok = true;
      } catch {
        ok = false;
      }
      if (!alive) return;
      if (ok && !onlineRef.current) {
        setOnline(true);
        reconnect.current?.();
      } else if (!ok && onlineRef.current) {
        setOnline(false);
      }
      // Poll fast while down (snappy recovery), slow while healthy.
      timer = setTimeout(tick, onlineRef.current ? 20_000 : 2_000);
    };

    tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, []);

  return { online, markOffline };
}
