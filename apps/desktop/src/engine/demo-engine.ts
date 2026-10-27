import type { UserId } from "../domain/types.ts";
import type { JoinRequest, MemberLevel, VoiceEngine } from "./engine.ts";

/**
 * Browser-only stand-in for the native engine, used when the UI runs outside Tauri
 * (`pnpm --filter @ourcamp/desktop dev`). Makes members "talk" in random turns so the
 * speaking ring can be designed without audio hardware. Produces no sound.
 */
export class DemoEngine implements VoiceEngine {
  #members: UserId[] = [];
  #timer: ReturnType<typeof setInterval> | null = null;
  readonly #listeners = new Set<(levels: readonly MemberLevel[]) => void>();
  #talking = new Map<UserId, number>();

  joinChannel(request: JoinRequest): Promise<void> {
    this.#members = request.members.map((m) => m.userId);
    this.#talking.clear();
    this.#timer ??= setInterval(() => this.#tick(), 100);
    return Promise.resolve();
  }

  leaveChannel(): Promise<void> {
    this.#members = [];
    if (this.#timer !== null) clearInterval(this.#timer);
    this.#timer = null;
    this.#emit([]);
    return Promise.resolve();
  }

  setVolume(): Promise<void> {
    return Promise.resolve();
  }

  setLocalMute(): Promise<void> {
    return Promise.resolve();
  }

  setMicMuted(): Promise<void> {
    return Promise.resolve();
  }

  setDeafened(): Promise<void> {
    return Promise.resolve();
  }

  onLevels(listener: (levels: readonly MemberLevel[]) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #tick(): void {
    for (const id of this.#members) {
      const left = (this.#talking.get(id) ?? 0) - 1;
      if (left > 0) this.#talking.set(id, left);
      else if (Math.random() < 0.03) this.#talking.set(id, 8 + Math.floor(Math.random() * 20));
      else this.#talking.delete(id);
    }
    this.#emit(
      this.#members.map((userId) => {
        const speaking = this.#talking.has(userId);
        return { userId, speaking, levelDb: speaking ? -22 : -70 };
      }),
    );
  }

  #emit(levels: readonly MemberLevel[]): void {
    for (const l of this.#listeners) l(levels);
  }
}
