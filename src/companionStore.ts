import { useSyncExternalStore } from "react";
import { emit, listen } from "@tauri-apps/api/event";

export type CompanionPet = { id: string; name: string; image: string };
export type CompanionPreferences = {
  petId: string;
  name: string;
  size: "small" | "medium" | "large";
  side: "left" | "right";
  motion: boolean;
  position: { x: number; y: number } | null;
  customPets: CompanionPet[];
};
export const BUILT_IN_PETS: CompanionPet[] = [
  { id: "nib", name: "Nib", image: "/pets/nib.png" },
  { id: "fold", name: "Fold", image: "/pets/fold.png" },
  { id: "byte", name: "Byte", image: "/pets/byte.png" },
  { id: "wisp", name: "Wisp", image: "/pets/wisp.png" },
  { id: "orbit", name: "Orbit", image: "/pets/orbit.png" },
  { id: "folio", name: "Folio", image: "/pets/folio.png" },
  { id: "loop", name: "Loop", image: "/pets/loop.png" },
  { id: "pip", name: "Pip", image: "/pets/pip.png" },
  { id: "glint", name: "Glint", image: "/pets/glint.png" },
  { id: "roam", name: "Roam", image: "/pets/roam.png" },
];
export const COMPANION_KEY = "noted-companion-v1";
const defaults: CompanionPreferences = { petId: "nib", name: "Nib", size: "medium", side: "right", motion: true, position: null, customPets: [] };

export function parseCompanion(value: string | null): CompanionPreferences {
  try {
    const data = JSON.parse(value ?? "null");
    if (!data || typeof data !== "object") return defaults;
    const customPets: CompanionPet[] = Array.isArray(data.customPets) ? data.customPets.filter((pet: CompanionPet) =>
      pet && typeof pet.id === "string" && pet.id.startsWith("custom-") &&
      typeof pet.name === "string" && pet.name.trim().length > 0 && pet.name.length <= 30 &&
      typeof pet.image === "string" && pet.image.length <= 1_400_000 && /^data:image\/png;base64,[A-Za-z0-9+/=]+$/.test(pet.image)
    ).slice(0, 8) : [];
    const pets = [...BUILT_IN_PETS, ...customPets];
    const selected = pets.find(pet => pet.id === data.petId) ?? BUILT_IN_PETS[0];
    return {
      petId: selected.id,
      name: typeof data.name === "string" && data.name.trim() ? data.name.trim().slice(0, 30) : selected.name,
      size: ["small", "medium", "large"].includes(data.size) ? data.size : "medium",
      side: data.side === "left" ? "left" : "right",
      motion: typeof data.motion === "boolean" ? data.motion : true,
      position: data.position && Number.isFinite(data.position.x) && Number.isFinite(data.position.y)
        ? { x: Math.max(0, Math.min(1, data.position.x)), y: Math.max(0, Math.min(1, data.position.y)) } : null,
      customPets,
    };
  } catch { return defaults; }
}

function readPreferences() {
  try { return parseCompanion(localStorage.getItem(COMPANION_KEY)); } catch { return defaults; }
}
let state = readPreferences();
const listeners = new Set<() => void>();
let nativeSyncStarted = false;
function startNativeSync() {
  if (nativeSyncStarted || typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
  nativeSyncStarted = true;
  void listen<CompanionPreferences>("companion-preferences", event => {
    const next = parseCompanion(JSON.stringify(event.payload));
    if (JSON.stringify(next) === JSON.stringify(state)) return;
    state = next;
    listeners.forEach(listener => listener());
  }).then(() => {
    if (new URLSearchParams(location.search).get("window") === "companion") return emit("companion-preferences-request", null);
  }).catch(() => {});
  if (new URLSearchParams(location.search).get("window") !== "companion") {
    void listen("companion-preferences-request", () => { void emit("companion-preferences", state).catch(() => {}); }).catch(() => {});
  }
}
const subscribe = (listener: () => void) => { startNativeSync(); listeners.add(listener); return () => { listeners.delete(listener); }; };
if (typeof window !== "undefined") window.addEventListener("storage", event => {
  if (event.key === COMPANION_KEY || event.key === null) {
    state = readPreferences();
    listeners.forEach(listener => listener());
  }
});

export function saveCompanion(patch: Partial<CompanionPreferences>) {
  const next = parseCompanion(JSON.stringify({ ...state, ...patch }));
  // Publish only after persistence succeeds, so the UI never claims an unsaved change.
  localStorage.setItem(COMPANION_KEY, JSON.stringify(next));
  state = next;
  listeners.forEach(listener => listener());
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) void emit("companion-preferences", next).catch(() => {});
}
export function useCompanion() {
  const preferences = useSyncExternalStore(subscribe, () => state);
  const pets = [...BUILT_IN_PETS, ...preferences.customPets];
  return { preferences, pets, pet: pets.find(pet => pet.id === preferences.petId) ?? BUILT_IN_PETS[0] };
}

export async function importPetImage(file: File): Promise<string> {
  if (!["image/png", "image/jpeg", "image/webp"].includes(file.type)) throw new Error("Choose a PNG, JPEG, or WebP image.");
  if (file.size > 5 * 1024 * 1024) throw new Error("Choose an image smaller than 5 MB.");
  const url = URL.createObjectURL(file);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    if (!img.naturalWidth || !img.naturalHeight || img.naturalWidth * img.naturalHeight > 40_000_000) throw new Error("Choose an image under 40 megapixels.");
    const scale = Math.min(1, 384 / Math.max(img.naturalWidth, img.naturalHeight));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(img.naturalWidth * scale));
    canvas.height = Math.max(1, Math.round(img.naturalHeight * scale));
    const context = canvas.getContext("2d");
    if (!context) throw new Error("Could not prepare this image.");
    context.drawImage(img, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/png");
  } finally { URL.revokeObjectURL(url); }
}
