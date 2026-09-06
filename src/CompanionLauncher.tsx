import { useEffect, useRef, useState, type CSSProperties, type RefObject, type PointerEvent as ReactPointerEvent } from "react";
import { MessageCircle } from "lucide-react";
import { api } from "./api";
import { useCompanion, saveCompanion } from "./companionStore";
import { useCompanionDesktop } from "./companionDesktop";
import { containPet, DRAG_SLOP, edgePull, PET_SIZES, rememberPet, restorePet, type PetState, type Point } from "./companionMotion";
import { useMotionPreference, usePetReaction } from "./useCompanionMotion";
import "./Companion.css";

export function usePetViewport() {
  const [viewport, setViewport] = useState({ width: innerWidth, height: innerHeight });
  useEffect(() => {
    const resize = () => setViewport({ width: innerWidth, height: innerHeight });
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  return viewport;
}
type Drag = { id: number; start: Point; grab: Point; origin: Point; last: Point; moved: boolean };

export function CompanionLauncher({ open, activity, onClick, onMove, buttonRef }: {
  open: boolean; activity: PetState; onClick: () => void; onMove: (position: Point) => void; buttonRef: RefObject<HTMLButtonElement | null>;
}) {
  const { preferences, pet } = useCompanion();
  const native = useCompanionDesktop();
  const viewport = usePetViewport();
  const size = PET_SIZES[preferences.size];
  const motion = useMotionPreference(preferences.motion);
  const [position, setPosition] = useState(() => restorePet(preferences.position, preferences.side, viewport, size));
  const [dragState, setDragState] = useState<PetState | null>(null);
  const [pull, setPull] = useState<ReturnType<typeof edgePull> | null>(null);
  const [error, setError] = useState("");
  const [handoff, setHandoff] = useState(false);
  const dragging = useRef<Drag | null>(null);
  const suppressed = useRef(false);
  const handingOff = useRef(false);
  const { state, react } = usePetReaction(activity, motion && !dragState && !open);
  const currentState = dragState ?? state;

  useEffect(() => {
    if (!dragging.current) setPosition(restorePet(preferences.position, preferences.side, viewport, size));
  }, [preferences.position, preferences.side, viewport, size]);
  useEffect(() => { onMove(position); }, [position, onMove]);
  useEffect(() => {
    const land = (event: Event) => {
      const point = (event as CustomEvent<Point>).detail;
      if (point) setPosition(restorePet(point, preferences.side, viewport, size));
      react("jumping");
    };
    window.addEventListener("companion-land", land);
    return () => window.removeEventListener("companion-land", land);
  });
  useEffect(() => {
    const cancel = () => {
      if (!dragging.current || handingOff.current) return;
      setPosition(dragging.current.origin);
      dragging.current = null;
      setDragState(null); setPull(null);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && dragging.current) { event.preventDefault(); event.stopImmediatePropagation(); cancel(); }
    };
    window.addEventListener("keydown", escape, true);
    window.addEventListener("blur", cancel);
    return () => { window.removeEventListener("keydown", escape, true); window.removeEventListener("blur", cancel); };
  }, []);

  function persist(point: Point) {
    try { saveCompanion({ position: rememberPet(point, viewport, size) }); }
    catch { setError("Moved for now. Couldn’t save this position."); }
  }
  async function detach(grab: Point) {
    handingOff.current = true; setHandoff(true); setPull(null);
    persist(position);
    try {
      await api.companionBeginDrag(grab.x, grab.y, size);
      dragging.current = null;
      setDragState(null);
      if (open) onClick();
    } catch {
      setError("Couldn’t move your pet to the desktop. Try again.");
      dragging.current = null; setDragState(null); react("failed");
    } finally { handingOff.current = false; setHandoff(false); }
  }
  function down(event: ReactPointerEvent<HTMLButtonElement>) {
    if (event.button !== 0 || handingOff.current || !event.isPrimary) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    event.currentTarget.setPointerCapture(event.pointerId);
    suppressed.current = false;
    setError("");
    dragging.current = { id: event.pointerId, start: { x: event.clientX, y: event.clientY },
      grab: { x: event.clientX - bounds.left, y: event.clientY - bounds.top }, origin: position,
      last: { x: event.clientX, y: event.clientY }, moved: false };
  }
  function move(event: ReactPointerEvent<HTMLButtonElement>) {
    const drag = dragging.current;
    if (!drag || drag.id !== event.pointerId || handingOff.current) return;
    const pointer = { x: event.clientX, y: event.clientY };
    if (!drag.moved && Math.hypot(pointer.x - drag.start.x, pointer.y - drag.start.y) < DRAG_SLOP) return;
    drag.moved = true; suppressed.current = true;
    const dx = pointer.x - drag.last.x;
    const edge = native.supported ? edgePull(pointer, drag.start, viewport) : null;
    setPull(edge && edge.progress > 0 ? edge : null);
    setDragState(edge && edge.progress > 0 ? "pulling" : Math.abs(dx) > 1 ? dx < 0 ? "drag-left" : "drag-right" : "dragged");
    drag.last = pointer;
    setPosition(containPet({ x: pointer.x - drag.grab.x, y: pointer.y - drag.grab.y }, viewport, size));
    if (edge?.ready) void detach(drag.grab);
  }
  function finish(event: ReactPointerEvent<HTMLButtonElement>, cancelled = false) {
    const drag = dragging.current;
    if (!drag || drag.id !== event.pointerId || handingOff.current) return;
    dragging.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    setDragState(null); setPull(null);
    if (cancelled) setPosition(drag.origin);
    else if (drag.moved) { persist(position); react("jumping"); }
  }
  if (native.detached) return null;
  const style = { left: position.x, top: position.y, "--pet-size": `${size}px`, "--pull": pull?.progress ?? 0 } as CSSProperties;
  return <>
    {pull && <div className={`companion-edge companion-edge-${pull.edge}`} style={{ "--pull": pull.progress } as CSSProperties} aria-hidden="true" />}
    <div className={`companion-launcher companion-free${motion ? " companion-animated" : ""}${dragState ? " is-dragging" : ""}`} style={style} data-state={currentState} data-edge={pull?.edge}>
      <button ref={buttonRef} className="companion-pet" onPointerDown={down} onPointerMove={move}
        onPointerUp={event => finish(event)} onPointerCancel={event => finish(event, true)} onLostPointerCapture={event => finish(event, true)}
        onClick={event => { if (suppressed.current && event.detail !== 0) { suppressed.current = false; return; } react("greeting"); onClick(); }}
        onPointerEnter={() => { if (!dragging.current && !open) react("greeting"); }}
        onKeyDown={event => {
          const steps: Record<string, Point> = { ArrowLeft: { x: -1, y: 0 }, ArrowRight: { x: 1, y: 0 }, ArrowUp: { x: 0, y: -1 }, ArrowDown: { x: 0, y: 1 } };
          if (steps[event.key]) {
            event.preventDefault(); const delta = steps[event.key], step = event.shiftKey ? 40 : 12;
            const next = containPet({ x: position.x + delta.x * step, y: position.y + delta.y * step }, viewport, size);
            setPosition(next); persist(next);
          }
          if (event.key.toLowerCase() === "j") { event.preventDefault(); react("jumping"); }
        }}
        aria-label={`${open ? "Close chat with" : "Ask"} ${preferences.name}`} aria-describedby="companion-drag-help"
        aria-haspopup="dialog" aria-expanded={open} aria-controls="companion-chat" disabled={handoff}>
        <span className="companion-body"><img src={pet.image} alt="" draggable={false} /></span>
        {!dragState && <span className={`companion-hint${position.y < 100 ? " hint-below" : ""}`}>{handoff ? "Heading outside…" : open ? "Close chat" : `Ask ${preferences.name} · drag to move`}</span>}
        <MessageCircle className="companion-chat-mark" size={16} aria-hidden="true" />
      </button>
      {pull && <div className={`companion-pull-label${position.y < 100 ? " hint-below" : ""}`} role="status">
        <span>{pull.ready ? "Off we go!" : "Pull to desktop"}</span><span className="companion-pull-track"><span style={{ width: `${pull.progress * 100}%` }} /></span>
      </div>}
      {error && <button className="companion-move-error" onClick={() => setError("")} role="alert">{error}</button>}
    </div>
    <span className="companion-sr-only" id="companion-drag-help">Drag to move. Arrow keys move your pet; hold Shift for larger steps. J to jump. Escape cancels a drag.</span>
  </>;
}
