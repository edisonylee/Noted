import { useEffect, useState } from "react";

// True on phone-sized viewports. Drives the mobile-first layout (bottom nav,
// dedicated capture screen). Layout keys off viewport — NOT `isDesktop` — so a
// resized desktop window behaves correctly too.
export function useIsMobile(breakpoint = 640): boolean {
  const query = `(max-width: ${breakpoint}px)`;
  const [isMobile, setIsMobile] = useState(
    () => typeof window !== "undefined" && window.matchMedia(query).matches
  );

  useEffect(() => {
    const mql = window.matchMedia(query);
    const onChange = () => setIsMobile(mql.matches);
    mql.addEventListener("change", onChange);
    onChange();
    return () => mql.removeEventListener("change", onChange);
  }, [query]);

  return isMobile;
}
