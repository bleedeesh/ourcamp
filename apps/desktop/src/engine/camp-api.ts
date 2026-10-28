import type { UserId } from "../domain/types.ts";

/** A joined camp, as listed in the switcher. */
export interface CampSummary {
  /** Stable id (the camp's certificate fingerprint). */
  readonly id: string;
  readonly name: string;
  readonly role: "member" | "admin";
}

export interface CampList {
  /** Name the user goes by; prefills the join form. */
  readonly displayName: string;
  readonly camps: readonly CampSummary[];
  /** Camp to open on start. */
  readonly current: string | null;
}

/** What an invite link leads to, shown before joining. */
export interface CampPreview {
  readonly name: string;
  /** Number of campfires. */
  readonly channels: number;
  readonly kind: "join" | "claim";
  /** `false` for an admin key somebody already redeemed. */
  readonly usable: boolean;
}

/** An opened camp. */
export interface OpenCamp {
  readonly id: string;
  readonly name: string;
  readonly channels: ReadonlyArray<{ readonly id: string; readonly name: string }>;
  readonly self: { readonly userId: UserId; readonly name: string; readonly isAdmin: boolean };
  /** The camp server was unreachable; name and channels are the last known ones. */
  readonly offline: boolean;
}

/**
 * Camps this device belongs to (ADR-0020). In the desktop app this is Rust
 * (`src-tauri/src/camps.rs`): it holds the device key and talks to camps over pinned
 * TLS. Every method rejects with a sentence ready to show the user.
 */
export interface CampApi {
  list(): Promise<CampList>;
  preview(link: string): Promise<CampPreview>;
  join(link: string, name: string): Promise<OpenCamp>;
  open(id: string): Promise<OpenCamp>;
  forget(id: string): Promise<void>;
  /** The camp's friends link, to copy. */
  invite(id: string): Promise<string>;
}
