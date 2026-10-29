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
    return { name: DEMO_CAMP.name, channels: DEMO_CAMP.channels.length, kind: link.includes("/claim/") ? "claim" : link.includes("/join/") ? "join" : "open", usable: !link.includes("/claim/") };
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

  leave(): Promise<boolean> {
    this.#joined = false;
    return Promise.resolve(true);
  }

  invite(): Promise<string> {
    return Promise.resolve(DEMO_LINK);
  }

  moveMember(): Promise<void> {
    // The demo's people are pictures: nobody's app is there to follow the move.
    return Promise.resolve();
  }

  #avatar: Uint8Array | null = null;

  profileAvatar(): Promise<Uint8Array | null> {
    return Promise.resolve(this.#avatar);
  }

  setProfileAvatar(picture: Uint8Array | null): Promise<void> {
    this.#avatar = picture;
    return Promise.resolve();
  }

  avatar(): Promise<Uint8Array> {
    // The demo's people have no pictures; they show their initials.
    return Promise.reject(new Error("no avatar in the demo"));
  }

  kick(): Promise<void> {
    return Promise.resolve();
  }

  setSilenced(): Promise<void> {
    return Promise.resolve();
  }

  onLive(): () => void {
    // The demo camp brings its people with it (see `main.tsx`); nothing goes live.
    return () => {};
  }

  onDirective(): () => void {
    // Nobody moderates the demo camp.
    return () => {};
  }

  #open(): OpenCamp {
    return {
      id: DEMO_ID,
      name: DEMO_CAMP.name,
      channels: DEMO_CAMP.channels.map((c) => ({ id: c.id, name: c.name, kind: "voice" as const, afk: c.afk })),
      self: { userId: DEMO_SELF_ID, name: this.#name || "Дима", role: "admin" as const },
      offline: false,
    };
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
