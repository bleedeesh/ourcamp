import type { UserId } from "./types.ts";

/**
 * Local, per-user listening volume.
 *
 * Mirrors `oc_audio::gain`: 100 % = as sent, 200 % = twice the amplitude (≈ +6 dB).
 * Volumes belong to the *listener* and follow the person across every camp, like
 * TeamSpeak — if Max is always too loud, he is turned down everywhere.
 */
export const MIN_VOLUME = 0;
export const MAX_VOLUME = 200;
export const DEFAULT_VOLUME = 100;
/** Slider step; fine enough to balance voices, coarse enough to hit 100 exactly. */
export const VOLUME_STEP = 5;

export function clampVolume(percent: number): number {
  if (!Number.isFinite(percent)) return DEFAULT_VOLUME;
  return Math.min(MAX_VOLUME, Math.max(MIN_VOLUME, Math.round(percent)));
}

export interface VolumePrefs {
  volume: number;
  /** Muted locally ("Заглушить для меня"); keeps `volume` for when it is undone. */
  muted: boolean;
}

export const DEFAULT_PREFS: VolumePrefs = Object.freeze({ volume: DEFAULT_VOLUME, muted: false });

/** Minimal key–value persistence; `localStorage` satisfies it, tests use a Map. */
export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

const STORAGE_KEY = "ourcamp.volumes.v1";

type Persisted = Record<UserId, VolumePrefs>;

/** Loads and saves per-user volume preferences, tolerating corrupt or foreign data. */
export class VolumeBook {
  readonly #storage: KeyValueStorage;
  #prefs: Persisted;

  constructor(storage: KeyValueStorage) {
    this.#storage = storage;
    this.#prefs = VolumeBook.#read(storage);
  }

  get(userId: UserId): VolumePrefs {
    return this.#prefs[userId] ?? DEFAULT_PREFS;
  }

  setVolume(userId: UserId, percent: number): VolumePrefs {
    return this.#write(userId, { ...this.get(userId), volume: clampVolume(percent) });
  }

  setMuted(userId: UserId, muted: boolean): VolumePrefs {
    return this.#write(userId, { ...this.get(userId), muted });
  }

  /** Back to 100 %, unmuted. */
  reset(userId: UserId): VolumePrefs {
    return this.#write(userId, DEFAULT_PREFS);
  }

  #write(userId: UserId, prefs: VolumePrefs): VolumePrefs {
    const isDefault = prefs.volume === DEFAULT_VOLUME && !prefs.muted;
    const next: Persisted = { ...this.#prefs };
    if (isDefault) delete next[userId];
    else next[userId] = prefs;
    this.#prefs = next;
    this.#storage.setItem(STORAGE_KEY, JSON.stringify(next));
    return this.get(userId);
  }

  static #read(storage: KeyValueStorage): Persisted {
    let raw: unknown;
    try {
      raw = JSON.parse(storage.getItem(STORAGE_KEY) ?? "{}");
    } catch {
      return {};
    }
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
    const out: Persisted = {};
    for (const [id, value] of Object.entries(raw)) {
      if (typeof value !== "object" || value === null) continue;
      const v = value as Partial<Record<keyof VolumePrefs, unknown>>;
      out[id] = {
        volume: clampVolume(typeof v.volume === "number" ? v.volume : DEFAULT_VOLUME),
        muted: v.muted === true,
      };
    }
    return out;
  }
}
