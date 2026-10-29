import { invoke } from "@tauri-apps/api/core";
import { subscribe } from "@ourcamp/ui/tauri-events.ts";

import type { AddressChoice } from "./address.ts";
import { STARTING, type Channel, type Role, type ServerStatus } from "./status.ts";

/**
 * What the window asks of the camp (src-tauri/src/lib.rs, manage.rs). Changes resolve
 * when saved — the window sees them through `onStatus` like everyone else — and reject
 * with a sentence ready to show.
 */
export interface ServerApi {
  status(): Promise<ServerStatus>;
  onStatus(listener: (status: ServerStatus) => void): () => void;
  retry(): Promise<void>;
  /** Keeps a friends address and restarts the camp with it (ADR-0037). */
  setAddress(choice: AddressChoice): Promise<void>;
  /** Looks for networks again (a VPN switched on after the start). */
  refreshAddresses(): Promise<void>;
  renameCamp(name: string): Promise<void>;
  setRole(userId: string, role: Role): Promise<void>;
  createChannel(name: string, kind: Channel["kind"]): Promise<void>;
  renameChannel(id: string, name: string): Promise<void>;
  deleteChannel(id: string): Promise<void>;
  /** Out of the camp, no ban: the friends link brings them back (ADR-0028). */
  kick(userId: string): Promise<void>;
  /** Someone in voice to another campfire. */
  moveMember(userId: string, channelId: string): Promise<void>;
  setSilenced(userId: string, silenced: boolean): Promise<void>;
  /** A member's avatar as a picture for <img> (ADR-0035); null without one. */
  avatar(userId: string): Promise<Blob | null>;
}

/** base64url → bytes (as `oc_proto::encode_bytes` writes them). */
function decodeBase64Url(text: string): Uint8Array<ArrayBuffer> {
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}

export function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export class TauriServerApi implements ServerApi {
  status(): Promise<ServerStatus> {
    return invoke<ServerStatus>("server_status");
  }

  onStatus(listener: (status: ServerStatus) => void): () => void {
    return subscribe<ServerStatus>("server://status", listener);
  }

  retry(): Promise<void> {
    return invoke("server_retry");
  }

  setAddress(choice: AddressChoice): Promise<void> {
    return invoke("server_set_address", { choice });
  }

  refreshAddresses(): Promise<void> {
    return invoke("server_refresh_addresses");
  }

  renameCamp(name: string): Promise<void> {
    return invoke("server_rename_camp", { name });
  }

  setRole(userId: string, role: Role): Promise<void> {
    return invoke("server_set_role", { userId, role });
  }

  createChannel(name: string, kind: Channel["kind"]): Promise<void> {
    return invoke("server_create_channel", { name, kind });
  }

  renameChannel(id: string, name: string): Promise<void> {
    return invoke("server_rename_channel", { id, name });
  }

  deleteChannel(id: string): Promise<void> {
    return invoke("server_delete_channel", { id });
  }

  kick(userId: string): Promise<void> {
    return invoke("server_kick", { userId });
  }

  moveMember(userId: string, channelId: string): Promise<void> {
    return invoke("server_move_member", { userId, channelId });
  }

  setSilenced(userId: string, silenced: boolean): Promise<void> {
    return invoke("server_set_silenced", { userId, silenced });
  }

  async avatar(userId: string): Promise<Blob | null> {
    const avatar = await invoke<{ mime: string; data: string } | null>("server_avatar", { userId });
    return avatar === null ? null : new Blob([decodeBase64Url(avatar.data)], { type: avatar.mime });
  }
}

/** Browser-only stand-in (`pnpm --filter @ourcamp/server dev`): a camp held in memory. */
export class DemoServerApi implements ServerApi {
  #status: ServerStatus = {
    ...STARTING,
    phase: { kind: "running" },
    campName: "Мой сервер",
    channels: [
      { id: "c0", name: "Voice", kind: "voice", afk: false },
      { id: "c1", name: "Chat", kind: "text", afk: false },
      { id: "afk", name: "AFK", kind: "voice", afk: true },
    ],
    address: "192.168.1.2:7780",
    fingerprint: "_Sr2o8XYvU2szmjofpL8y3VsZdOgO-e0cqwXOJZqBP8",
    joinLink: "ourcamp://192.168.1.2/join/k3j4h5g6f7d8s9a0?fp=_Sr2o8XYvU2szmjofpL8y3VsZdOgO-e0cqwXOJZqBP8",
    members: [
      { userId: "u1", name: "bleedeesh", role: "admin", campfire: "Voice", campfireId: "c0", micMuted: false, silenced: false, avatar: null },
      { userId: "u2", name: "angelina", role: "moderator", campfire: "Voice", campfireId: "c0", micMuted: true, silenced: false, avatar: null },
      { userId: "u3", name: "Артём", role: "member", campfire: null, campfireId: null, micMuted: false, silenced: true, avatar: null },
    ],
    livekitPid: 18244,
    reach: {
      choice: { kind: "auto" },
      options: [
        { kind: "lan", ip: "192.168.1.2" },
        { kind: "radmin", ip: "26.12.34.56" },
      ],
      note: null,
    },
  };
  readonly #listeners = new Set<(status: ServerStatus) => void>();
  #nextId = 1;

  status(): Promise<ServerStatus> {
    return Promise.resolve(this.#status);
  }

  onStatus(listener: (status: ServerStatus) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  retry(): Promise<void> {
    return Promise.resolve();
  }

  setAddress(choice: AddressChoice): Promise<void> {
    if (choice.kind === "custom" && !/^[a-z0-9.-]+$/i.test(choice.host.trim())) {
      return Promise.reject(new Error("Введите домен (mycamp.duckdns.org) или IP (85.12.34.56)"));
    }
    const host =
      choice.kind === "custom"
        ? choice.host.trim().toLowerCase()
        : (this.#status.reach.options.find((o) => choice.kind === "detected" && o.kind === choice.which) ??
            this.#status.reach.options[0])?.ip ?? "127.0.0.1";
    return this.#change({
      ...this.#status,
      address: `${host}:7780`,
      joinLink: `ourcamp://${host}/join/k3j4h5g6f7d8s9a0?fp=_Sr2o8XYvU2szmjofpL8y3VsZdOgO-e0cqwXOJZqBP8`,
      reach: { ...this.#status.reach, choice, note: null },
    });
  }

  refreshAddresses(): Promise<void> {
    return Promise.resolve();
  }

  renameCamp(name: string): Promise<void> {
    return this.#change({ ...this.#status, campName: name.trim() });
  }

  setRole(userId: string, role: Role): Promise<void> {
    const members = this.#status.members.map((m) => (m.userId === userId ? { ...m, role } : m));
    return this.#change({ ...this.#status, members });
  }

  createChannel(name: string, kind: Channel["kind"]): Promise<void> {
    const channels = [...this.#status.channels];
    const afk = channels.findIndex((c) => c.afk);
    channels.splice(afk === -1 ? channels.length : afk, 0, { id: `demo${this.#nextId++}`, name: name.trim(), kind, afk: false });
    return this.#change({ ...this.#status, channels });
  }

  renameChannel(id: string, name: string): Promise<void> {
    const channels = this.#status.channels.map((c) => (c.id === id ? { ...c, name: name.trim() } : c));
    return this.#change({ ...this.#status, channels });
  }

  deleteChannel(id: string): Promise<void> {
    if (this.#status.channels.find((c) => c.id === id)?.afk) {
      return Promise.reject(new Error("Канал AFK удалить нельзя — его можно только переименовать"));
    }
    return this.#change({ ...this.#status, channels: this.#status.channels.filter((c) => c.id !== id) });
  }

  kick(userId: string): Promise<void> {
    return this.#change({ ...this.#status, members: this.#status.members.filter((m) => m.userId !== userId) });
  }

  moveMember(userId: string, channelId: string): Promise<void> {
    const channel = this.#status.channels.find((c) => c.id === channelId);
    if (channel?.kind !== "voice") return Promise.reject(new Error("Перенести можно только в голосовой канал"));
    const members = this.#status.members.map((m) =>
      m.userId === userId ? { ...m, campfire: channel.name, campfireId: channel.id } : m,
    );
    return this.#change({ ...this.#status, members });
  }

  setSilenced(userId: string, silenced: boolean): Promise<void> {
    const members = this.#status.members.map((m) => (m.userId === userId ? { ...m, silenced } : m));
    return this.#change({ ...this.#status, members });
  }

  avatar(): Promise<Blob | null> {
    // The demo's people have no pictures.
    return Promise.resolve(null);
  }

  #change(status: ServerStatus): Promise<void> {
    this.#status = status;
    for (const l of this.#listeners) l(status);
    return Promise.resolve();
  }
}
