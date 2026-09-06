import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { orgPath, team } from "./client";
import { initials } from "./presentation";
import type { TeamUser } from "./types";

const AvatarCache = createContext<Map<string, Promise<string>> | null>(null);
export function TeamAvatars({ children }: { children: ReactNode }) {
  const cache = useRef(new Map<string, Promise<string>>());
  return (
    <AvatarCache.Provider value={cache.current}>
      {children}
    </AvatarCache.Provider>
  );
}
export function TeamAvatar({
  org,
  person,
  className = "",
}: {
  org: string;
  person: TeamUser;
  className?: string;
}) {
  const cache = useContext(AvatarCache);
  const [photo, setPhoto] = useState<{ key: string; data: string } | null>(
    null,
  );
  const key = `${org}:${person.id}:${person.avatar_version ?? ""}`;
  useEffect(() => {
    if (!person.avatar_version) return;
    let active = true;
    let request = cache?.get(key);
    if (!request) {
      request = team
        .request<{ data: string }>(
          "GET",
          orgPath(org, `/profiles/${person.id}/avatar`),
        )
        .then((value) => value.data);
      if (cache && cache.size >= 256) cache.delete(cache.keys().next().value!);
      cache?.set(key, request);
    }
    request
      .then((data) => {
        if (active) setPhoto({ key, data });
      })
      .catch(() => {
        cache?.delete(key);
      });
    return () => {
      active = false;
    };
  }, [org, person.id, person.avatar_version, key, cache]);
  const data = photo?.key === key ? photo.data : "";
  return (
    <span className={`team-avatar ${className}`} aria-hidden="true">
      {data ? <img src={data} alt="" /> : initials(person.name)}
    </span>
  );
}
