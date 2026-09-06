import { useEffect, useRef, useState } from "react";
import { Camera, Loader } from "lucide-react";
import { team } from "./client";
import { initials } from "./presentation";
import { TeamDialog } from "./TeamDialog";
import { TeamAvatar } from "./TeamAvatar";
import type { TeamProfile, TeamUser } from "./types";

async function preparePhoto(file: File) {
  if (
    !["image/jpeg", "image/png", "image/webp"].includes(file.type) ||
    file.size > 10_000_000
  )
    throw new Error("Choose a JPEG, PNG, or WebP photo smaller than 10 MB.");
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    image.src = url;
    await image.decode();
    if (!image.naturalWidth || !image.naturalHeight)
      throw new Error("This photo could not be opened.");
    const canvas = document.createElement("canvas");
    canvas.width = 192;
    canvas.height = 192;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("This photo could not be prepared.");
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, 192, 192);
    const side = Math.min(image.naturalWidth, image.naturalHeight);
    ctx.drawImage(
      image,
      (image.naturalWidth - side) / 2,
      (image.naturalHeight - side) / 2,
      side,
      side,
      0,
      0,
      192,
      192,
    );
    return canvas.toDataURL("image/jpeg", 0.86);
  } finally {
    URL.revokeObjectURL(url);
  }
}
export function TeamProfileSettings({
  onSaved,
}: {
  onSaved: () => Promise<unknown>;
}) {
  const [profile, setProfile] = useState<TeamProfile | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [photoBusy, setPhotoBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [retry, setRetry] = useState(0);
  const file = useRef<HTMLInputElement>(null);
  useEffect(() => {
    let active = true;
    team
      .request<TeamProfile>("GET", "/v1/profile")
      .then((value) => {
        if (active) {
          setProfile(value);
          setError("");
        }
      })
      .catch((e) => {
        if (active) setError(String(e));
      });
    return () => {
      active = false;
    };
  }, [retry]);
  const edit = (value: Partial<TeamProfile>) => {
    setSaved(false);
    setProfile((old) => (old ? { ...old, ...value } : old));
  };
  return (
    <section className="team-profile-settings">
      <h2>Your profile</h2>
      <p className="team-muted">
        How you appear to members of your teams on this server.
      </p>
      {error && (
        <p role="alert" className="team-error">
          {error}{" "}
          <button
            className="team-text-button"
            onClick={() => setRetry((v) => v + 1)}
          >
            Reload profile
          </button>
        </p>
      )}
      {!profile && !error && <p role="status">Loading your profile…</p>}
      {profile && (
        <form
          className="team-form"
          onSubmit={async (event) => {
            event.preventDefault();
            if (busy || photoBusy) return;
            setBusy(true);
            setError("");
            setSaved(false);
            try {
              const value = await team.request<TeamProfile>(
                "PATCH",
                "/v1/profile",
                profile,
              );
              setProfile(value);
              await onSaved();
              setSaved(true);
            } catch (error) {
              setError(String(error));
            } finally {
              setBusy(false);
            }
          }}
        >
          <div className="team-profile-photo-edit">
            <span className="team-avatar profile-photo">
              {profile.avatar_data ? (
                <img src={profile.avatar_data} alt="Your profile photo" />
              ) : (
                initials(profile.name)
              )}
            </span>
            <div>
              <input
                ref={file}
                className="team-profile-file"
                type="file"
                aria-label="Choose profile photo"
                accept="image/jpeg,image/png,image/webp"
                disabled={busy || photoBusy}
                onChange={async (event) => {
                  const next = event.target.files?.[0];
                  event.target.value = "";
                  if (!next) return;
                  setPhotoBusy(true);
                  setError("");
                  try {
                    edit({ avatar_data: await preparePhoto(next) });
                  } catch (error) {
                    setError(String(error));
                  } finally {
                    setPhotoBusy(false);
                  }
                }}
              />
              <button
                type="button"
                className="team-text-button"
                disabled={busy || photoBusy}
                onClick={() => file.current?.click()}
              >
                <Camera size={16} />
                {photoBusy ? "Preparing photo…" : "Change photo"}
              </button>
              {profile.avatar_data && (
                <button
                  type="button"
                  className="team-text-button"
                  disabled={busy || photoBusy}
                  onClick={() => edit({ avatar_data: "" })}
                >
                  Remove photo
                </button>
              )}
              <p className="team-muted">Square crop · JPEG, PNG, or WebP</p>
            </div>
          </div>
          <label>
            Display name
            <input
              required
              maxLength={100}
              value={profile.name}
              disabled={busy}
              onChange={(e) => edit({ name: e.target.value })}
            />
          </label>
          <label>
            Job title <span className="team-muted">optional</span>
            <input
              maxLength={120}
              placeholder="Product designer"
              value={profile.title ?? ""}
              disabled={busy}
              onChange={(e) => edit({ title: e.target.value })}
            />
          </label>
          <label>
            About you <span className="team-muted">optional</span>
            <textarea
              rows={3}
              maxLength={500}
              placeholder="What you work on, or how you like to collaborate."
              value={profile.about ?? ""}
              disabled={busy}
              onChange={(e) => edit({ about: e.target.value })}
            />
          </label>
          <div className="team-profile-save">
            <button
              className="team-primary"
              disabled={busy || photoBusy || !profile.name.trim()}
            >
              {busy && <Loader size={14} className="spin" />} Save profile
            </button>
            {saved && <span role="status">Profile saved.</span>}
          </div>
        </form>
      )}
    </section>
  );
}
export function TeamProfileCard({
  org,
  person,
  onClose,
}: {
  org: string;
  person: TeamUser;
  onClose: () => void;
}) {
  return (
    <TeamDialog title={person.name} onClose={onClose}>
      <div className="team-profile-card">
        <TeamAvatar org={org} person={person} className="profile-photo" />
        {person.title && <p className="team-profile-title">{person.title}</p>}
        {person.about ? (
          <p className="team-profile-about">{person.about}</p>
        ) : (
          <p className="team-muted">This teammate hasn’t added a bio yet.</p>
        )}
      </div>
    </TeamDialog>
  );
}
