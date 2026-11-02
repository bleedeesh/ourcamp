/** The longest side of a photo sent to a chat (ADR-0046): sharp on a screen, small to send. */
export const MAX_PHOTO_SIDE = 1600;
/** The largest photo a camp keeps (`oc_proto::MAX_PHOTO_BYTES`). */
export const MAX_PHOTO_BYTES = 2 * 1024 * 1024;
/** The largest GIF a camp keeps (`oc_proto::MAX_GIF_BYTES`). */
export const MAX_GIF_BYTES = 8 * 1024 * 1024;
/** The largest side a camp lets a picture claim (`oc_proto::MAX_PHOTO_SIDE`). */
export const MAX_GIF_SIDE = 4096;

/** `width`×`height` scaled down to fit `max` on its long side; never up, never to zero. */
export function fitWithin(width: number, height: number, max = MAX_PHOTO_SIDE): { width: number; height: number } {
  const scale = Math.min(1, max / Math.max(width, height));
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/** The media type by the file's own signature — what the camp checks too. */
export function photoType(bytes: Uint8Array): "image/webp" | "image/png" | "image/jpeg" | "image/gif" | null {
  const at = (i: number) => bytes[i];
  if (bytes.length >= 12 && at(0) === 0x52 && at(1) === 0x49 && at(2) === 0x46 && at(3) === 0x46) {
    const webp = String.fromCharCode(...bytes.subarray(8, 12));
    if (webp === "WEBP") return "image/webp";
  }
  if (bytes.length >= 8 && at(0) === 0x89 && at(1) === 0x50 && at(2) === 0x4e && at(3) === 0x47) return "image/png";
  if (bytes.length >= 3 && at(0) === 0xff && at(1) === 0xd8 && at(2) === 0xff) return "image/jpeg";
  if (bytes.length >= 6) {
    const gif = String.fromCharCode(...bytes.subarray(0, 6));
    if (gif === "GIF87a" || gif === "GIF89a") return "image/gif";
  }
  return null;
}

/** Why a GIF cannot go, as a sentence; `null` when it can. It goes as it is, so it cannot be made smaller. */
export function gifRefusal(bytes: number, width: number, height: number): string | null {
  if (bytes > MAX_GIF_BYTES) return `GIF весит больше ${sizeLabel(MAX_GIF_BYTES)} — такой отправить нельзя`;
  if (Math.max(width, height) > MAX_GIF_SIDE) return `GIF слишком большой: можно до ${MAX_GIF_SIDE} точек по длинной стороне`;
  return null;
}

/** "1,4 МБ", "380 КБ". */
export function sizeLabel(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1).replace(".", ",")} МБ`;
  return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
}
