import { invoke } from "@tauri-apps/api/core";
import { subscribe } from "@ourcamp/ui/tauri-events.ts";

import { STARTING, type ServerStatus } from "./status.ts";

/** What the window asks of the camp (src-tauri/src/lib.rs). */
export interface ServerApi {
  status(): Promise<ServerStatus>;
  onStatus(listener: (status: ServerStatus) => void): () => void;
  retry(): Promise<void>;
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
}

/** Browser-only stand-in (`pnpm --filter @ourcamp/server dev`) to design the window. */
export class DemoServerApi implements ServerApi {
  status(): Promise<ServerStatus> {
    return Promise.resolve({
      ...STARTING,
      phase: { kind: "running" },
      campName: "Мой лагерь",
      address: "192.168.1.2:7780",
      fingerprint: "_Sr2o8XYvU2szmjofpL8y3VsZdOgO-e0cqwXOJZqBP8",
      joinLink: "ourcamp://192.168.1.2/join/k3j4h5g6f7d8s9a0?fp=_Sr2o8XYvU2szmjofpL8y3VsZdOgO-e0cqwXOJZqBP8",
      claimLink: null,
      members: [
        { userId: "u1", name: "bleedeesh", isAdmin: true, campfire: "Лобби", micMuted: false },
        { userId: "u2", name: "angelina", isAdmin: false, campfire: "Лобби", micMuted: true },
        { userId: "u3", name: "Артём", isAdmin: false, campfire: null, micMuted: false },
      ],
      livekitPid: 18244,
    });
  }

  onStatus(): () => void {
    return () => {};
  }

  retry(): Promise<void> {
    return Promise.resolve();
  }
}
