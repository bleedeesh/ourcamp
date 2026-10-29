import { useEffect, useState } from "react";

import type { ServerApi } from "../api.ts";
import type { Member } from "../status.ts";

/** Pictures by avatar hash, for the life of the window: each is fetched once. */
const urls = new Map<string, Promise<string | null>>();

function pictureOf(api: ServerApi, member: Member, hash: string): Promise<string | null> {
  let url = urls.get(hash);
  if (!url) {
    url = api.avatar(member.userId).then((blob) => (blob === null ? null : URL.createObjectURL(blob)));
    urls.set(hash, url);
    // A failed fetch is not remembered: the next status asks again.
    url.catch(() => urls.delete(hash));
  }
  return url;
}

/** A member's avatar (ADR-0035), or their initial until it arrives or without one. */
export function MemberAvatar({ member, size, api }: { member: Member; size: number; api: ServerApi }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    const hash = member.avatar;
    if (hash === null) return setUrl(null);
    let live = true;
    pictureOf(api, member, hash).then(
      (made) => live && setUrl(made),
      () => live && setUrl(null),
    );
    return () => {
      live = false;
    };
  }, [api, member]);
  return (
    <span className="avatar" style={{ width: size, height: size, fontSize: Math.round(size * 0.42) }} aria-hidden="true">
      {url === null ? member.name.slice(0, 1).toUpperCase() : <img className="avatar__picture" src={url} alt="" draggable={false} />}
    </span>
  );
}
