import { useEffect, useRef, useState } from "react";
import { Check, Upload } from "lucide-react";
import { api } from "./api";
import { useCompanionDesktop } from "./companionDesktop";
import { PET_SIZES } from "./companionMotion";
import { importPetImage, saveCompanion, useCompanion, type CompanionPreferences } from "./companionStore";
import "./Companion.css";

export { CompanionLauncher } from "./CompanionLauncher";

export function CompanionSettings() {
  const { preferences, pets, pet } = useCompanion();
  const desktop = useCompanionDesktop();
  const [name, setName] = useState(preferences.name);
  useEffect(() => setName(preferences.name), [preferences.name]);
  const [error, setError] = useState("");
  const [importing, setImporting] = useState(false);
  const uploadRef = useRef<HTMLInputElement>(null);
  function update(patch: Partial<CompanionPreferences>) {
    try { saveCompanion(patch); setError(""); }
    catch { setError("Couldn’t save your companion. Local storage may be full. Try removing a custom pet."); }
  }
  function saveName() {
    const nextName = name.trim() || pet.name;
    update({
      name: nextName,
      ...(pet.id.startsWith("custom-") ? {
        customPets: preferences.customPets.map(item => item.id === pet.id ? { ...item, name: nextName } : item),
      } : {}),
    });
    setName(nextName);
  }
  async function upload(file: File) {
    setImporting(true);
    setError("");
    try {
      if (preferences.customPets.length >= 8) throw new Error("You can keep up to 8 custom pets. Remove one before adding another.");
      const image = await importPetImage(file);
      const name = file.name.replace(/\.[^.]+$/, "").replace(/[-_]/g, " ").trim().slice(0, 30) || "My pet";
      const custom = { id: `custom-${crypto.randomUUID()}`, name, image };
      saveCompanion({ petId: custom.id, name, customPets: [...preferences.customPets, custom] });
    } catch (reason) { setError(reason instanceof Error ? reason.message : "Couldn’t add this pet."); }
    finally { setImporting(false); }
  }
  return (
    <section className="companion-settings" aria-label="Companion preferences">
      <div className="companion-intro">
        <img src={pet.image} alt="" />
        <div><h4>A little company.</h4><p>Your assistant, with a face of its own. Click your pet anytime to ask about your notes and meetings.</p></div>
      </div>
      <div className="companion-pet-list" role="group" aria-label="Choose a pet">
        {pets.map(option => <button key={option.id} className="companion-choice" aria-pressed={preferences.petId === option.id}
          disabled={importing} onClick={() => update({ petId: option.id, name: option.name })}>
          <img src={option.image} alt="" /><span>{option.name}</span>
          {preferences.petId === option.id && <Check size={13} aria-hidden="true" />}
        </button>)}
      </div>
      <div className="companion-upload-row">
        <button className="link" onClick={() => uploadRef.current?.click()} disabled={importing || preferences.customPets.length >= 8}>
          <Upload size={14} />{importing ? "Adding pet…" : "Add your own pet"}
        </button>
        {pet.id.startsWith("custom-") && <button className="link" disabled={importing} onClick={() => update({
          customPets: preferences.customPets.filter(item => item.id !== pet.id), petId: "nib", name: "Nib",
        })}>Remove this pet</button>}
        <input ref={uploadRef} type="file" accept="image/png,image/jpeg,image/webp" hidden onChange={event => {
          const file = event.target.files?.[0]; event.target.value = ""; if (file) void upload(file);
        }} />
      </div>
      <p className="companion-help">Draw a pet or bring your own artwork. PNG, JPEG, or WebP, up to 5 MB. Transparent backgrounds work best. Keep up to 8 custom pets on this device.</p>
      <label className="field"><span className="field-label">Name</span>
        <input value={name} maxLength={30} aria-label="Companion name" disabled={importing}
          onChange={event => setName(event.target.value)} onBlur={saveName}
          onKeyDown={event => { if (event.key === "Enter" || event.key === "Escape") saveName(); }} />
      </label>
      <div className="companion-field-row">
        <label className="field"><span className="field-label">Size</span><select aria-label="Size" value={preferences.size} onChange={event => update({ size: event.target.value as CompanionPreferences["size"] })}>
          <option value="small">Small</option><option value="medium">Medium</option><option value="large">Large</option>
        </select></label>
        <label className="field"><span className="field-label">Home corner</span><select aria-label="Corner" value={preferences.side} onChange={event => update({ side: event.target.value as CompanionPreferences["side"], position: null })}>
          <option value="right">Bottom right</option><option value="left">Bottom left</option>
        </select></label>
      </div>
      <div className="companion-upload-row">
        <button className="link" onClick={() => update({ position: null })}>Reset position</button>
        {desktop.detached && <button className="link" onClick={() => void api.companionReturn().catch(() => setError("Couldn’t bring your pet home. Try again."))}>Bring pet back to Noted</button>}
        {desktop.supported && !desktop.detached && <button className="link" onClick={() => {
          const size = PET_SIZES[preferences.size];
          void api.companionBeginDrag(size / 2, size / 2, size, false).catch(() => setError("Couldn’t move your pet to the desktop. Try again."));
        }}>Move to desktop</button>}
      </div>
      <label className="companion-motion"><input type="checkbox" checked={preferences.motion} onChange={event => update({ motion: event.target.checked })} />Pet animations</label>
      <p className="companion-help">Drag your pet anywhere. {desktop.supported ? "Pull a little further at an edge to take it onto your desktop. Drag it back over Noted to bring it home. " : ""}Animations follow your system’s reduced motion preference.</p>
      {error && <p className="companion-error" role="alert">{error}</p>}
    </section>
  );
}
