import { createStore, type Store } from "../lib/store.ts";
import type { MemberLevel, VoiceEngine } from "../engine/engine.ts";
import type { Camp, UserId } from "./types.ts";
import type { VolumeBook, VolumePrefs } from "./volume.ts";

export interface VolumeMenu {
  readonly userId: UserId;
  /** Viewport coordinates the menu is anchored to. */
  readonly x: number;
  readonly y: number;
}

export interface VoiceState {
  readonly camp: Camp;
  readonly selfId: UserId;
  /** Voice channel the local user is in, or `null` when not connected. */
  readonly channelId: string | null;
  readonly micMuted: boolean;
  readonly deafened: boolean;
  /** Who is speaking right now (only `true` entries are kept). */
  readonly speaking: Readonly<Record<UserId, true>>;
  /** Local volume prefs of members the user has seen in this session. */
  readonly prefs: Readonly<Record<UserId, VolumePrefs>>;
  readonly volumeMenu: VolumeMenu | null;
  /** Last engine error, shown to the user. */
  readonly error: string | null;
}

/**
 * Owns voice-related UI state and keeps the native engine in sync with it.
 *
 * The UI only calls these methods; it never talks to the engine directly. State is
 * updated optimistically and rolled back with an error message if the engine fails.
 */
export class VoiceController {
  readonly store: Store<VoiceState>;
  readonly #engine: VoiceEngine;
  readonly #volumes: VolumeBook;
  /** Mic state before deafening, restored on undeafen (deafen implies mute). */
  #micBeforeDeafen = false;
  #unsubscribeLevels: (() => void) | null = null;

  constructor(engine: VoiceEngine, volumes: VolumeBook, camp: Camp, selfId: UserId) {
    this.#engine = engine;
    this.#volumes = volumes;
    this.store = createStore<VoiceState>({
      camp,
      selfId,
      channelId: null,
      micMuted: false,
      deafened: false,
      speaking: {},
      prefs: {},
      volumeMenu: null,
      error: null,
    });
  }

  /** Starts listening to engine level updates. Idempotent. */
  start(): void {
    this.#unsubscribeLevels ??= this.#engine.onLevels((levels) => this.#applyLevels(levels));
  }

  dispose(): void {
    this.#unsubscribeLevels?.();
    this.#unsubscribeLevels = null;
  }

  async join(channelId: string): Promise<void> {
    const { camp, channelId: current, selfId } = this.store.get();
    if (current === channelId) return;
    const channel = camp.channels.find((c) => c.id === channelId);
    if (!channel) throw new Error(`unknown channel ${channelId}`);

    const remote = channel.memberIds.filter((id) => id !== selfId);
    const prefs = Object.fromEntries(remote.map((id) => [id, this.#volumes.get(id)]));
    this.store.patch({ channelId, speaking: {}, prefs: { ...this.store.get().prefs, ...prefs }, error: null });
    await this.#run(
      () =>
        this.#engine.joinChannel({
          campId: camp.id,
          channelId,
          members: remote.map((userId) => ({
            userId,
            ...this.#volumes.get(userId),
            micMuted: camp.members[userId]?.micMuted ?? false,
          })),
        }),
      () => this.store.patch({ channelId: current }),
    );
  }

  async leave(): Promise<void> {
    const { channelId } = this.store.get();
    if (channelId === null) return;
    this.store.patch({ channelId: null, speaking: {}, volumeMenu: null });
    await this.#run(
      () => this.#engine.leaveChannel(),
      () => this.store.patch({ channelId }),
    );
  }

  async toggleMic(): Promise<void> {
    const { micMuted, deafened } = this.store.get();
    if (deafened) {
      // Unmuting while deafened means "I want to talk": undeafen and unmute together.
      this.store.patch({ deafened: false, micMuted: false });
      await this.#run(async () => {
        await this.#engine.setDeafened(false);
        await this.#engine.setMicMuted(false);
      }, () => this.store.patch({ deafened: true, micMuted: true }));
      return;
    }
    this.store.patch({ micMuted: !micMuted });
    await this.#run(
      () => this.#engine.setMicMuted(!micMuted),
      () => this.store.patch({ micMuted }),
    );
  }

  async toggleDeafen(): Promise<void> {
    const { deafened, micMuted } = this.store.get();
    if (!deafened) {
      this.#micBeforeDeafen = micMuted;
      this.store.patch({ deafened: true, micMuted: true });
      await this.#run(async () => {
        await this.#engine.setDeafened(true);
        await this.#engine.setMicMuted(true);
      }, () => this.store.patch({ deafened: false, micMuted }));
    } else {
      const restoreMic = this.#micBeforeDeafen;
      this.store.patch({ deafened: false, micMuted: restoreMic });
      await this.#run(async () => {
        await this.#engine.setDeafened(false);
        await this.#engine.setMicMuted(restoreMic);
      }, () => this.store.patch({ deafened: true, micMuted: true }));
    }
  }

  prefsOf(userId: UserId): VolumePrefs {
    return this.store.get().prefs[userId] ?? this.#volumes.get(userId);
  }

  async setVolume(userId: UserId, percent: number): Promise<void> {
    const prefs = this.#volumes.setVolume(userId, percent);
    this.#setPrefs(userId, prefs);
    await this.#run(() => this.#engine.setVolume(userId, prefs.volume));
  }

  async toggleLocalMute(userId: UserId): Promise<void> {
    const prefs = this.#volumes.setMuted(userId, !this.prefsOf(userId).muted);
    this.#setPrefs(userId, prefs);
    await this.#run(() => this.#engine.setLocalMute(userId, prefs.muted));
  }

  async resetVolume(userId: UserId): Promise<void> {
    const prefs = this.#volumes.reset(userId);
    this.#setPrefs(userId, prefs);
    await this.#run(async () => {
      await this.#engine.setVolume(userId, prefs.volume);
      await this.#engine.setLocalMute(userId, false);
    });
  }

  openVolumeMenu(menu: VolumeMenu): void {
    if (menu.userId === this.store.get().selfId) return;
    this.#setPrefs(menu.userId, this.prefsOf(menu.userId));
    this.store.patch({ volumeMenu: menu });
  }

  closeVolumeMenu(): void {
    this.store.patch({ volumeMenu: null });
  }

  dismissError(): void {
    this.store.patch({ error: null });
  }

  #setPrefs(userId: UserId, prefs: VolumePrefs): void {
    this.store.update((s) => ({ ...s, prefs: { ...s.prefs, [userId]: prefs } }));
  }

  #applyLevels(levels: readonly MemberLevel[]): void {
    const current = this.store.get().speaking;
    const next: Record<UserId, true> = {};
    for (const l of levels) if (l.speaking) next[l.userId] = true;
    const changed =
      Object.keys(next).length !== Object.keys(current).length ||
      Object.keys(next).some((id) => !(id in current));
    // Levels arrive ~20×/s; only touch the store when the set of speakers changes.
    if (changed) this.store.patch({ speaking: next });
  }

  async #run(action: () => Promise<void>, rollback?: () => void): Promise<void> {
    try {
      await action();
    } catch (err) {
      rollback?.();
      this.store.patch({ error: err instanceof Error ? err.message : String(err) });
    }
  }
}
