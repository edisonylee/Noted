import {
  useEffect,
  useRef,
  type RefObject,
  type PointerEvent,
  type MouseEvent,
} from "react";
import { outsideBounds } from "./dismissal";

/** Require an outside press AND click: dragging from content onto the backdrop is not dismissal. */
export function useBackdropDismiss(
  onDismiss: () => void,
  disabled = false,
  nativeDialog = false,
) {
  const startedOutside = useRef(false);
  const outside = (
    event: PointerEvent<HTMLElement> | MouseEvent<HTMLElement>,
  ) =>
    event.target === event.currentTarget &&
    (!nativeDialog ||
      outsideBounds(event, event.currentTarget.getBoundingClientRect()));
  return {
    onPointerDown: (event: PointerEvent<HTMLElement>) => {
      startedOutside.current = event.button === 0 && outside(event);
    },
    onPointerCancel: () => {
      startedOutside.current = false;
    },
    onClick: (event: MouseEvent<HTMLElement>) => {
      const dismiss = startedOutside.current && outside(event);
      startedOutside.current = false;
      if (dismiss && !disabled) {
        event.stopPropagation();
        onDismiss();
      }
    },
  };
}
const layers: symbol[] = [];
/** Include both the popup and its trigger so clicking the trigger can still toggle it. */
export function useOutsideDismiss(
  open: boolean,
  boundaries: RefObject<HTMLElement | null>[],
  onDismiss: (reason: "outside" | "escape") => void,
) {
  const latest = useRef({ boundaries, onDismiss });
  latest.current = { boundaries, onDismiss };
  useEffect(() => {
    if (!open) return;
    const layer = Symbol("dismissible layer");
    layers.push(layer);
    const eligible = () => {
      if (layers[layers.length - 1] !== layer) return false;
      const dialogs =
        document.querySelectorAll<HTMLDialogElement>("dialog[open]");
      const modal = dialogs[dialogs.length - 1];
      return (
        !modal ||
        latest.current.boundaries.some(
          (ref) => ref.current && modal.contains(ref.current),
        )
      );
    };
    const pointer = (event: globalThis.PointerEvent) => {
      if (event.button !== 0 || !eligible()) return;
      if (
        !latest.current.boundaries.some(
          (ref) => ref.current && event.composedPath().includes(ref.current),
        )
      )
        latest.current.onDismiss("outside");
    };
    const key = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || !eligible())
        return;
      event.preventDefault();
      event.stopPropagation();
      const focusTargets = latest.current.boundaries.map((ref) =>
        ref.current?.matches("button, summary")
          ? ref.current
          : ref.current?.querySelector<HTMLElement>("summary, button"),
      );
      latest.current.onDismiss("escape");
      requestAnimationFrame(() =>
        focusTargets.find((target) => target?.isConnected)?.focus(),
      );
    };
    document.addEventListener("pointerdown", pointer, true);
    document.addEventListener("keydown", key);
    return () => {
      layers.splice(layers.indexOf(layer), 1);
      document.removeEventListener("pointerdown", pointer, true);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
}
