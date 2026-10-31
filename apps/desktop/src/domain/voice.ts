import { createStore, type Store } from "../lib/store.ts";
import type { LinkStatus, MemberLevel, ParticipantsUpdate, VoiceEngine } from "../engine/engine.ts";
import type { CampState } from "../engine/camp-api.ts";
import { applyCampState, applyRoster } from "./roster.ts";
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
  /** Connection quality and ping in the channel (ADR-0030); null until measured. */
  readonly link: LinkStatus | null;
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
  /** Mic and sound before entering the AFK campfire, restored on the way out (ADR-0025). */
  #beforeAfk: { readonly micMuted: boolean; readonly deafened: boolean } | null = null;
  #unsubscribeLevels: (() => void) | null = null;
  #unsubscribeParticipants: (() => void) | null = null;
  #unsubscribeDisconnected: (() => void) | null = null;
  #unsubscribeLink: (() => void) | null = null;
  /** Members whose saved volume the engine already has for the current channel. */
  #volumesSent = new Set<UserId>();

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
      link: null,
      error: null,
    });
  }

  /** Starts listening to engine level and membership updates. Idempotent. */
  start(): void {
    this.#unsubscribeLevels ??= this.#engine.onLevels((levels) => this.#applyLevels(levels));
    this.#unsubscribeParticipants ??= this.#engine.onParticipants((update) => this.#applyParticipants(update));
    this.#unsubscribeDisconnected ??= this.#engine.onDisconnected((reason) => this.#onDisconnected(reason));
    this.#unsubscribeLink ??= this.#engine.onLink((link) => {
      // A report racing a leave must not bring the line back.
      if (this.store.get().channelId !== null) this.store.patch({ link });
    });
  }

  dispose(): void {
    this.#unsubscribeLevels?.();
    this.#unsubscribeLevels = null;
    this.#unsubscribeParticipants?.();
    this.#unsubscribeParticipants = null;
    this.#unsubscribeDisconnected?.();
    this.#unsubscribeDisconnected = null;
    this.#unsubscribeLink?.();
    this.#unsubscribeLink = null;
  }

  /** Switches to another camp: leaves the current campfire first. */
  async openCamp(camp: Camp, selfId: UserId): Promise<void> {
    if (this.store.get().channelId !== null) await this.leave();
    this.#volumesSent = new Set();
    this.store.patch({ camp, selfId, channelId: null, speaking: {}, volumeMenu: null });
  }

  async join(channelId: string): Promise<void> {
    const { camp, channelId: current, selfId } = this.store.get();
    if (current === channelId) return;
    const channel = camp.channels.find((c) => c.id === channelId);
    if (!channel) throw new Error(`unknown channel ${channelId}`);

    // AFK: mic and sound off before the room connects, so nothing leaks either way.
    if (channel.afk && this.#beforeAfk === null) {
      const { micMuted, deafened } = this.store.get();
      this.#beforeAfk = { micMuted, deafened };
      this.#micBeforeDeafen = true; // undeafening inside AFK keeps the mic off
      await this.#setSound(true, true);
    }

    const remote = channel.memberIds.filter((id) => id !== selfId);
    const prefs = Object.fromEntries(remote.map((id) => [id, this.#volumes.get(id)]));
    this.#volumesSent = new Set(remote);
    // A new room measures its own connection.
    this.store.patch({ channelId, speaking: {}, link: null, prefs: { ...this.store.get().prefs, ...prefs }, error: null });
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
    if (!channel.afk) await this.#leaveAfk();
  }

  async leave(): Promise<void> {
    const { channelId } = this.store.get();
    if (channelId === null) return;
    this.store.patch({ channelId: null, speaking: {}, volumeMenu: null, link: null });
    await this.#run(
      () => this.#engine.leaveChannel(),
      () => this.store.patch({ channelId }),
    );
    await this.#leaveAfk();
  }

  /** Back from AFK: mic and sound as they were before it. */
  async #leaveAfk(): Promise<void> {
    const before = this.#beforeAfk;
    if (before === null) return;
    this.#beforeAfk = null;
    this.#micBeforeDeafen = before.micMuted;
    await this.#setSound(before.micMuted, before.deafened);
  }

  /** Sets mic and sound together, rolling both back if the engine refuses. */
  async #setSound(micMuted: boolean, deafened: boolean): Promise<void> {
    const previous = { micMuted: this.store.get().micMuted, deafened: this.store.get().deafened };
    this.store.patch({ micMuted, deafened });
    await this.#run(async () => {
      await this.#engine.setDeafened(deafened);
      await this.#engine.setMicMuted(micMuted);
    }, () => this.store.patch(previous));
  }

  /** A moderator forbade you to speak (ADR-0028): the mic button stays locked. */
  silenced(): boolean {
    const { camp, selfId } = this.store.get();
    return camp.members[selfId]?.silenced === true;
  }

  async toggleMic(): Promise<void> {
    // The camp's LiveKit would refuse the microphone anyway; the button says why.
    if (this.silenced()) return;
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

  /** The camp server's snapshot: who sits where, names and crowns (ADR-0021). */
  applyCampState(state: CampState): void {
    const { camp, channelId, selfId } = this.store.get();
    this.store.patch({ camp: applyCampState(camp, state, channelId, selfId) });
  }

  /** The server ended our session: we are no longer at the campfire; say why. */
  #onDisconnected(reason: string): void {
    if (this.store.get().channelId === null) return;
    this.store.patch({ channelId: null, speaking: {}, volumeMenu: null, link: null, error: reason });
    void this.#leaveAfk();
  }

  #applyParticipants(update: ParticipantsUpdate): void {
    const { camp, channelId } = this.store.get();
    // Leaving announces an empty room. Until the camp server reports presence (M3)
    // the last known list is the best information we have, so keep it.
    if (channelId === null) return;
    this.store.patch({ camp: applyRoster(camp, channelId, update), selfId: update.selfId });

    // People who arrived after we joined: the engine has no saved volume for them yet.
    for (const { userId } of update.members) {
      if (this.#volumesSent.has(userId)) continue;
      this.#volumesSent.add(userId);
      const prefs = this.#volumes.get(userId);
      this.#setPrefs(userId, prefs);
      void this.#run(async () => {
        await this.#engine.setVolume(userId, prefs.volume);
        await this.#engine.setLocalMute(userId, prefs.muted);
      });
    }
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

  /**
   * Opens `userId`'s stream. Streams are watched only from their channel, so someone in
   * another one comes over first (ADR-0049); if that fails, the error is already shown.
   */
  async watchStream(userId: UserId, channelId: string): Promise<void> {
    const { camp, selfId } = this.store.get();
    const member = camp.members[userId];
    if (!member || userId === selfId) return;
    await this.join(channelId);
    if (this.store.get().channelId !== channelId) return;
    await this.#run(() => this.#engine.watchStream(userId, member.name));
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
