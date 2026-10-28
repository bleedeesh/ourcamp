// Fakes shared by the domain tests (not itself a test file).
import type { JoinRequest, MemberLevel, ParticipantsUpdate, VoiceEngine } from "../engine/engine.ts";
import type { KeyValueStorage } from "./volume.ts";

export class MemoryStorage implements KeyValueStorage {
  readonly data = new Map<string, string>();
  getItem = (k: string) => this.data.get(k) ?? null;
  setItem = (k: string, v: string) => void this.data.set(k, v);
}

/** Records calls; can be told to fail the next call. */
export class FakeEngine implements VoiceEngine {
  calls: string[] = [];
  joined: JoinRequest | null = null;
  failNext = false;
  #listener: ((l: readonly MemberLevel[]) => void) | null = null;

  #record(call: string): Promise<void> {
    this.calls.push(call);
    if (this.failNext) {
      this.failNext = false;
      return Promise.reject(new Error("device lost"));
    }
    return Promise.resolve();
  }
  joinChannel(r: JoinRequest) {
    this.joined = r;
    return this.#record(`join:${r.channelId}`);
  }
  leaveChannel() {
    return this.#record("leave");
  }
  setVolume(id: string, p: number) {
    return this.#record(`volume:${id}:${p}`);
  }
  setLocalMute(id: string, m: boolean) {
    return this.#record(`localMute:${id}:${m}`);
  }
  setMicMuted(m: boolean) {
    return this.#record(`mic:${m}`);
  }
  setDeafened(d: boolean) {
    return this.#record(`deaf:${d}`);
  }
  onLevels(listener: (l: readonly MemberLevel[]) => void) {
    this.#listener = listener;
    return () => (this.#listener = null);
  }
  emit(levels: MemberLevel[]) {
    this.#listener?.(levels);
  }
  #participants: ((u: ParticipantsUpdate) => void) | null = null;
  onParticipants(listener: (u: ParticipantsUpdate) => void) {
    this.#participants = listener;
    return () => (this.#participants = null);
  }
  announce(update: ParticipantsUpdate) {
    this.#participants?.(update);
  }
  #disconnected: ((reason: string) => void) | null = null;
  onDisconnected(listener: (reason: string) => void) {
    this.#disconnected = listener;
    return () => (this.#disconnected = null);
  }
  disconnect(reason: string) {
    this.#disconnected?.(reason);
  }
}
