import { useEffect, useSyncExternalStore } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { api, isDesktop } from "./api";
import { saveCompanion } from "./companionStore";
import type { PetState, Point } from "./companionMotion";

type DesktopStatus = Awaited<ReturnType<typeof api.companionDesktopStatus>>;
let status: DesktopStatus = { supported: false, detached: false, dragging: false, overApp: false, direction: 0 };
let activity: PetState = "idle";
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
const notify = () => listeners.forEach(listener => listener());
let started = false;
function start() {
  if (started || !isDesktop) return;
  started = true;
  void listen<DesktopStatus>("companion-desktop-state", event => { status = event.payload; notify(); })
    .then(() => api.companionDesktopStatus()).then(next => { status = next; notify(); }).catch(() => {});
  void listen<PetState>("companion-activity", event => { activity = event.payload; notify(); })
    .then(() => {
      if (new URLSearchParams(location.search).get("window") === "companion") return emit("companion-activity-request", null);
    }).catch(() => {});
  if (new URLSearchParams(location.search).get("window") !== "companion") {
    void listen("companion-activity-request", () => { void emit("companion-activity", activity).catch(() => {}); }).catch(() => {});
  }
  void listen<Point>("companion-returned", event => {
    try { saveCompanion({ position: event.payload }); } catch { /* The next drag can still move the pet. */ }
    window.dispatchEvent(new CustomEvent("companion-land", { detail: event.payload }));
  }).catch(() => {});
}
export function useCompanionDesktop() {
  useEffect(start, []);
  return useSyncExternalStore(subscribe, () => status);
}
export function useDesktopActivity() {
  useEffect(start, []);
  return useSyncExternalStore(subscribe, () => activity);
}
export function publishPetActivity(next: PetState) {
  activity = next;
  notify();
  if (isDesktop) void emit("companion-activity", next).catch(() => {});
}
