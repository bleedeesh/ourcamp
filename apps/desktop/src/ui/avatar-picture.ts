import { AVATAR_SIZE, cropSquare, MAX_AVATAR_BYTES } from "../domain/avatar.ts";

/** Encodings to try, best first: WebP where the webview can make it, else PNG. */
const ENCODINGS: ReadonlyArray<readonly [type: string, quality?: number]> = [
  ["image/webp", 0.85],
  ["image/webp", 0.7],
  ["image/png"],
];

function toBlob(canvas: HTMLCanvasElement, type: string, quality?: number): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, type, quality));
}

/**
 * Turns a picked file into an avatar (ADR-0035): the centred square at 128×128, as WebP
 * (PNG where the webview cannot encode WebP), within the camp's limit. The webview does
 * the decoding and scaling: no image library in the app. Rejects with a sentence.
 */
export async function makeAvatar(file: Blob): Promise<Uint8Array> {
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new Error("Это не картинка — выберите PNG, JPEG, WebP или GIF");
  }
  const { sx, sy, side } = cropSquare(bitmap.width, bitmap.height);
  const canvas = document.createElement("canvas");
  canvas.width = AVATAR_SIZE;
  canvas.height = AVATAR_SIZE;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("Не получилось обработать картинку");
  context.imageSmoothingQuality = "high";
  context.drawImage(bitmap, sx, sy, side, side, 0, 0, AVATAR_SIZE, AVATAR_SIZE);
  bitmap.close();
  for (const [type, quality] of ENCODINGS) {
    const blob = await toBlob(canvas, type, quality);
    // A webview that cannot make WebP silently returns PNG: check what came out.
    if (blob && blob.type === type && blob.size <= MAX_AVATAR_BYTES) {
      return new Uint8Array(await blob.arrayBuffer());
    }
  }
  throw new Error("Картинка слишком сложная — попробуйте другую");
}
