import { useEffect, useMemo, useState } from "react";
import { avatarFormat } from "../domain/avatar.ts";
import { useCampsController } from "./camps-context.tsx";

/** Object URLs by avatar hash, for the life of the window: a picture is decoded once. */
const urls = new Map<string, string>();

/** An object URL for avatar bytes — only for PNG/WebP, as the camp keeps (ADR-0035). */
export function pictureUrl(bytes: Uint8Array): string | null {
  const type = avatarFormat(bytes);
  return type === null ? null : URL.createObjectURL(new Blob([bytes as BlobPart], { type }));
}

/** The picture of a member's avatar (by the snapshot's hash), or `null` while there is none. */
export function useAvatarUrl(userId: string, hash: string | null): string | null {
  const camps = useCampsController();
  const [url, setUrl] = useState<string | null>(() => (hash === null ? null : (urls.get(hash) ?? null)));
  useEffect(() => {
    if (hash === null) return setUrl(null);
    const known = urls.get(hash);
    if (known) return setUrl(known);
    let live = true;
    camps.avatarOf(userId, hash).then(
      (bytes) => {
        const made = urls.get(hash) ?? pictureUrl(bytes);
        if (made !== null) urls.set(hash, made);
        if (live) setUrl(made);
      },
      // Unreachable camp or a stale hash: the initial stays; the next snapshot retries.
      () => {
        if (live) setUrl(null);
      },
    );
    return () => {
      live = false;
    };
  }, [camps, userId, hash]);
  return url;
}

/** An object URL for bytes the component owns (your avatar in the join form); freed after. */
export function useBytesUrl(bytes: Uint8Array | null): string | null {
  const url = useMemo(() => (bytes === null ? null : pictureUrl(bytes)), [bytes]);
  useEffect(() => () => {
    if (url !== null) URL.revokeObjectURL(url);
  }, [url]);
  return url;
}
