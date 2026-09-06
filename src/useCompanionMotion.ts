import { useEffect, useState } from "react";
import type { PetState } from "./companionMotion";

export function useMotionPreference(enabled: boolean) {
  const [reduced, setReduced] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);
  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const sync = () => setReduced(media.matches);
    media.addEventListener("change", sync);
    return () => media.removeEventListener("change", sync);
  }, []);
  return enabled && !reduced;
}

export function usePetReaction(base: PetState, enabled: boolean) {
  const [reaction, setReaction] = useState<{ state: PetState; id: number } | null>(null);
  useEffect(() => {
    if (!reaction) return;
    const timer = window.setTimeout(() => setReaction(null), reaction.state === "jumping" ? 840 : 980);
    return () => clearTimeout(timer);
  }, [reaction]);
  useEffect(() => {
    if (!enabled || base !== "idle") return;
    // An occasional small hop adds life without wandering across the document.
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") setReaction({ state: "jumping", id: Date.now() });
    }, 32000);
    return () => clearInterval(timer);
  }, [base, enabled]);
  return { state: base === "idle" ? reaction?.state ?? base : base, react: (state: PetState) => setReaction({ state, id: Date.now() }) };
}
