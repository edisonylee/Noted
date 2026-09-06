import { useEffect, useRef, useState } from "react";
import { CornerDownLeft } from "lucide-react";
import { api } from "./api";
import { useCompanion } from "./companionStore";
import { useCompanionDesktop, useDesktopActivity } from "./companionDesktop";
import { DRAG_SLOP, PET_SIZES, type Point } from "./companionMotion";
import { useMotionPreference, usePetReaction } from "./useCompanionMotion";
import "./Companion.css";

export function DesktopCompanion() {
  const { preferences, pet } = useCompanion();
  const desktop = useCompanionDesktop();
  const activity = useDesktopActivity();
  const motion = useMotionPreference(preferences.motion);
  const { state, react } = usePetReaction(activity, motion && desktop.detached && !desktop.dragging);
  const size = PET_SIZES[preferences.size];
  const [error, setError] = useState("");
  const down = useRef<{ point: Point; grab: Point } | null>(null);
  const suppressed = useRef(false);
  const wasDragging = useRef(false);
  useEffect(() => {
    if (wasDragging.current && !desktop.dragging) react("jumping");
    wasDragging.current = desktop.dragging;
  }, [desktop.dragging]);
  const home = () => void api.companionReturn().catch(() => setError("Couldn’t return. Try again."));
  return <div className={`desktop-companion${motion ? " companion-animated" : ""}`} data-state={desktop.dragging ? desktop.direction === 0 ? "dragged" : desktop.direction < 0 ? "drag-left" : "drag-right" : state}>
    <button className="companion-pet" style={{ width: size, height: size, left: (168 - size) / 2, top: 44 }}
      aria-label={`Ask ${preferences.name}`} title={`Ask ${preferences.name} · drag to move · Escape to return`}
      onPointerDown={event => {
        if (event.button !== 0 || !event.isPrimary) return;
        const rect = event.currentTarget.getBoundingClientRect();
        down.current = { point: { x: event.clientX, y: event.clientY }, grab: { x: event.clientX - rect.left, y: event.clientY - rect.top } };
        suppressed.current = false;
        event.currentTarget.setPointerCapture(event.pointerId);
      }} onPointerMove={event => {
        const start = down.current;
        if (!start || Math.hypot(event.clientX - start.point.x, event.clientY - start.point.y) < DRAG_SLOP) return;
        down.current = null; suppressed.current = true;
        void api.companionBeginDrag(start.grab.x, start.grab.y, size).catch(() => setError("Couldn’t move. Try again."));
      }} onPointerUp={() => { down.current = null; }} onPointerCancel={() => { down.current = null; }}
      onClick={event => { if (suppressed.current && event.detail !== 0) { suppressed.current = false; return; } void api.companionOpenChat().catch(() => setError("Couldn’t open Noted.")); }}
      onPointerEnter={() => { if (!desktop.dragging) react("greeting"); }}
      onKeyDown={event => { if (event.key === "Escape") home(); if (event.key.toLowerCase() === "j") react("jumping"); }}>
      <span className="companion-body"><img src={pet.image} alt="" draggable={false} /></span>
    </button>
    {desktop.dragging ? <span className="desktop-companion-status">{desktop.overApp ? "Release to come home" : "Exploring…"}</span>
      : <button className="desktop-companion-home" onClick={home}><CornerDownLeft size={12} /> Back to Noted</button>}
    {error && <button className="desktop-companion-error" role="alert" onClick={() => setError("")}>{error}</button>}
  </div>;
}
