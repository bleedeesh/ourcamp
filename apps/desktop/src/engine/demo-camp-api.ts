import { DEMO_CAMP, DEMO_SELF_ID } from "../domain/demo-camp.ts";
import type { CampApi, CampList, CampPreview, OpenCamp } from "./camp-api.ts";

const DEMO_ID = "demo";
const DEMO_LINK = "ourcamp://192.0.2.15/join/7k2q9m4x8v1c3b5n?fp=" + "A".repeat(43);

/**
 * Browser-only stand-in for the camp commands (`pnpm --filter @ourcamp/desktop dev`):
 * starts with no camps so the first-run screens can be designed, and any well-formed
 * `ourcamp://` link "joins" the demo camp.
 */
export class DemoCampApi implements CampApi {
  #joined = false;
  #name = "";

  list(): Promise<CampList> {
    return Promise.resolve({
      displayName: this.#name,
      camps: this.#joined ? [{ id: DEMO_ID, name: DEMO_CAMP.name, role: "admin" as const }] : [],
      current: this.#joined ? DEMO_ID : null,
    });
  }

  async preview(link: string): Promise<CampPreview> {
    await delay(300);
    if (!link.trim().toLowerCase().startsWith("ourcamp://")) throw new Error("Это не ссылка OurCamp");
    return { name: DEMO_CAMP.name, channels: DEMO_CAMP.channels.length, kind: link.includes("/claim/") ? "claim" : "join", usable: true };
  }

  async join(_link: string, name: string): Promise<OpenCamp> {
    await delay(300);
    this.#joined = true;
    this.#name = name.trim();
    return this.#open();
  }

  open(): Promise<OpenCamp> {
    return Promise.resolve(this.#open());
  }

  forget(): Promise<void> {
    this.#joined = false;
    return Promise.resolve();
  }

  invite(): Promise<string> {
    return Promise.resolve(DEMO_LINK);
  }

  #open(): OpenCamp {
    return {
      id: DEMO_ID,
      name: DEMO_CAMP.name,
      channels: DEMO_CAMP.channels.map((c) => ({ id: c.id, name: c.name })),
      self: { userId: DEMO_SELF_ID, name: this.#name || "Дима", isAdmin: true },
      offline: false,
    };
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
