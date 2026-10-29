/** The longest side of a photo sent to a chat (ADR-0046): sharp on a screen, small to send. */
export const MAX_PHOTO_SIDE = 1600;
/** The largest photo a camp keeps (`oc_proto::MAX_PHOTO_BYTES`). */
export const MAX_PHOTO_BYTES = 2 * 1024 * 1024;

/** `width`×`height` scaled down to fit `max` on its long side; never up, never to zero. */
export function fitWithin(width: number, height: number, max = MAX_PHOTO_SIDE): { width: number; height: number } {
  const scale = Math.min(1, max / Math.max(width, height));
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/** The media type by the file's own signature — what the camp checks too. */
export function photoType(bytes: Uint8Array): "image/webp" | "image/png" | "image/jpeg" | null {
  const at = (i: number) => bytes[i];
  if (bytes.length >= 12 && at(0) === 0x52 && at(1) === 0x49 && at(2) === 0x46 && at(3) === 0x46) {
    const webp = String.fromCharCode(...bytes.subarray(8, 12));
    if (webp === "WEBP") return "image/webp";
  }
  if (bytes.length >= 8 && at(0) === 0x89 && at(1) === 0x50 && at(2) === 0x4e && at(3) === 0x47) return "image/png";
  if (bytes.length >= 3 && at(0) === 0xff && at(1) === 0xd8 && at(2) === 0xff) return "image/jpeg";
  return null;
}

/** "1,4 МБ", "380 КБ". */
export function sizeLabel(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1).replace(".", ",")} МБ`;
  return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
}
