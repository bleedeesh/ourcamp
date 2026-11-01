import type { OutgoingPhoto } from "../../engine/camp-api.ts";
import { fitWithin, gifRefusal, MAX_PHOTO_BYTES, photoType } from "../../domain/photo.ts";

/** Encodings to try, best first: WebP where the webview makes it, then JPEG. */
const ENCODINGS: ReadonlyArray<readonly [type: string, quality: number]> = [
  ["image/webp", 0.85],
  ["image/webp", 0.72],
  ["image/jpeg", 0.85],
  ["image/jpeg", 0.7],
];

function toBlob(canvas: HTMLCanvasElement, type: string, quality: number): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, type, quality));
}

/**
 * Turns a picked, pasted or dropped file into a photo for a chat (ADR-0046): at most
 * 1600 px on its long side, re-encoded — which also leaves the camera's metadata
 * (location included) behind. The webview decodes and scales: no image library in the
 * app. A GIF goes byte for byte: a canvas would keep only its first frame (ADR-0076).
 * Rejects with a sentence.
 */
export async function makePhoto(file: Blob): Promise<OutgoingPhoto> {
  let bitmap: ImageBitmap;
  try {
    // Turned as the camera meant it (EXIF orientation), then the metadata is gone.
    bitmap = await createImageBitmap(file, { imageOrientation: "from-image" });
  } catch {
    throw new Error("Это не картинка — подойдёт JPEG, PNG, WebP или GIF");
  }
  if (photoType(new Uint8Array(await file.slice(0, 6).arrayBuffer())) === "image/gif") {
    const { width, height } = bitmap;
    bitmap.close();
    const refusal = gifRefusal(file.size, width, height);
    if (refusal !== null) throw new Error(refusal);
    return { bytes: new Uint8Array(await file.arrayBuffer()), width, height };
  }
  const { width, height } = fitWithin(bitmap.width, bitmap.height);
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d");
  if (!context) throw new Error("Не получилось обработать картинку");
  context.imageSmoothingQuality = "high";
  context.drawImage(bitmap, 0, 0, width, height);
  bitmap.close();
  for (const [type, quality] of ENCODINGS) {
    const blob = await toBlob(canvas, type, quality);
    // A webview that cannot make WebP silently returns PNG: check what came out.
    if (blob && blob.type === type && blob.size <= MAX_PHOTO_BYTES) {
      return { bytes: new Uint8Array(await blob.arrayBuffer()), width, height };
    }
  }
  throw new Error("Фото слишком большое даже после сжатия — попробуйте другое");
}
