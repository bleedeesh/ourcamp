import { createContext, useContext, useEffect, useState } from "react";
import { avatarFormat } from "../domain/avatar.ts";

/** Fetches a member's avatar by the snapshot's hash: the main window's camps or the chat's. */
export type AvatarSource = (userId: string, hash: string) => Promise<Uint8Array>;

/** Where avatars come from in this window; without one, everyone shows their initial. */
export const AvatarSourceContext = createContext<AvatarSource | null>(null);

/** Object URLs by avatar hash, for the life of the window: a picture is decoded once. */
const urls = new Map<string, string>();

/** An object URL for avatar bytes — only for PNG/WebP, as the camp keeps (ADR-0035). */
export function pictureUrl(bytes: Uint8Array): string | null {
  const type = avatarFormat(bytes);
  return type === null ? null : URL.createObjectURL(new Blob([bytes as BlobPart], { type }));
}

/** The picture of a member's avatar (by the snapshot's hash), or `null` while there is none. */
export function useAvatarUrl(userId: string, hash: string | null): string | null {
  const source = useContext(AvatarSourceContext);
  const [url, setUrl] = useState<string | null>(() => (hash === null ? null : (urls.get(hash) ?? null)));
  useEffect(() => {
    if (hash === null || source === null) return setUrl(null);
    const known = urls.get(hash);
    if (known) return setUrl(known);
    let live = true;
    source(userId, hash).then(
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
  }, [source, userId, hash]);
  return url;
}

/** An object URL for bytes the component owns (your avatar in the join form); freed after. */
export function useBytesUrl(bytes: Uint8Array | null): string | null {
  return useObjectUrl(bytes, avatarFormat);
}

/**
 * An object URL for bytes the component owns, of the type `typeOf` reads from them
 * (`null`: not a picture); revoked when the bytes change or the component goes.
 * Made and revoked in one effect: React's development double run of effects then
 * revokes the first URL and makes a second, instead of leaving a dead one in place.
 */
export function useObjectUrl(bytes: Uint8Array | null, typeOf: (bytes: Uint8Array) => string | null): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    const type = bytes === null ? null : typeOf(bytes);
    if (bytes === null || type === null) {
      setUrl(null);
      return;
    }
    const made = URL.createObjectURL(new Blob([bytes as BlobPart], { type }));
    setUrl(made);
    return () => URL.revokeObjectURL(made);
  }, [bytes, typeOf]);
  return url;
}
