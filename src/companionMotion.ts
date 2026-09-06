export type Point = { x: number; y: number };
export type Viewport = { width: number; height: number };
export type PetState = "idle" | "greeting" | "dragged" | "drag-left" | "drag-right" | "pulling" | "jumping" | "working" | "waiting" | "review" | "failed";
export type Edge = "left" | "right" | "top" | "bottom";
export const PET_SIZES = { small: 64, medium: 84, large: 104 };
export const DRAG_SLOP = 6;
export const EDGE_ZONE = 80;
export const EXIT_PULL = 72;
export const clamp = (n: number, min: number, max: number) => Math.max(min, Math.min(Math.max(min, max), n));

export function containPet(point: Point, viewport: Viewport, size: number): Point {
  return { x: clamp(point.x, 8, viewport.width - size - 8), y: clamp(point.y, 8, viewport.height - size - 8) };
}
export function restorePet(position: Point | null, side: "left" | "right", viewport: Viewport, size: number): Point {
  return containPet(position ? { x: position.x * (viewport.width - size), y: position.y * (viewport.height - size) }
    : { x: side === "left" ? 20 : viewport.width - size - 20, y: viewport.height - size - 14 }, viewport, size);
}
export function rememberPet(point: Point, viewport: Viewport, size: number): Point {
  return { x: clamp(point.x / Math.max(1, viewport.width - size), 0, 1), y: clamp(point.y / Math.max(1, viewport.height - size), 0, 1) };
}
export function edgePull(pointer: Point, start: Point, viewport: Viewport) {
  const distances: [Edge, number][] = [["left", pointer.x], ["right", viewport.width - pointer.x], ["top", pointer.y], ["bottom", viewport.height - pointer.y]];
  const [edge, distance] = distances.sort((a, b) => a[1] - b[1])[0];
  const outward = edge === "left" ? start.x - pointer.x : edge === "right" ? pointer.x - start.x : edge === "top" ? start.y - pointer.y : pointer.y - start.y;
  const progress = clamp((EDGE_ZONE - distance) / EXIT_PULL, 0, 1);
  // A pet already near an edge must still be deliberately pulled outwards.
  return { edge, progress: outward > DRAG_SLOP ? progress : 0, ready: progress >= 1 && outward >= 24 };
}
export function panelNearPet(point: Point, viewport: Viewport, size: number, width: number, height: number) {
  const actualHeight = Math.min(height, viewport.height - 24);
  const above = point.y - actualHeight - 12;
  return { left: clamp(point.x + size - width, 12, viewport.width - width - 12),
    top: clamp(above >= 12 ? above : point.y + size + 12, 12, viewport.height - actualHeight - 12),
    maxHeight: viewport.height - 24 };
}
