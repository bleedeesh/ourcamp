// Avatars (ADR-0035): the pure part — the square to cut, the size and format rules the
// camp checks too (`oc_proto::AvatarFormat`, `MAX_AVATAR_BYTES`), and base64url for IPC.

/** The largest avatar a camp keeps. */
export const MAX_AVATAR_BYTES = 96 * 1024;
/** Side of the square the app sends, in pixels. */
export const AVATAR_SIZE = 128;

/** The centred square of a `width`×`height` picture: what a round avatar shows. */
export function cropSquare(width: number, height: number): { sx: number; sy: number; side: number } {
  const side = Math.min(width, height);
  return { sx: Math.floor((width - side) / 2), sy: Math.floor((height - side) / 2), side };
}

/** The picture's format by its signature, as the camp decides it; `null`: refused there. */
export function avatarFormat(bytes: Uint8Array): "image/webp" | "image/png" | null {
  const ascii = (from: number, to: number) => String.fromCharCode(...bytes.subarray(from, to));
  if (bytes.length >= 12 && ascii(0, 4) === "RIFF" && ascii(8, 12) === "WEBP") return "image/webp";
  const png = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  if (bytes.length >= png.length && png.every((b, i) => bytes[i] === b)) return "image/png";
  return null;
}

/** Bytes → base64url without padding, as `oc_proto::encode_bytes` writes it. */
export function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  // Chunks: spreading a whole picture into one call would overflow the argument stack.
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** base64url (padded or not) → bytes. Throws on anything else. */
export function decodeBase64Url(text: string): Uint8Array {
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}
