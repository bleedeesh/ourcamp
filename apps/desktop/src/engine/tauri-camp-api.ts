import { invoke } from "@tauri-apps/api/core";
import type { CampApi, CampList, CampLive, CampPreview, OpenCamp } from "./camp-api.ts";
import { subscribe } from "@ourcamp/ui/tauri-events.ts";

/** Event emitted by `src-tauri/src/lib.rs` (`CAMP_STATE_EVENT`). */
const CAMP_STATE_EVENT = "camp://state";

/** Camp commands of `src-tauri/src/commands.rs`. */
export class TauriCampApi implements CampApi {
  list(): Promise<CampList> {
    return invoke("camp_list");
  }

  preview(link: string): Promise<CampPreview> {
    return invoke("camp_preview", { link });
  }

  join(link: string, name: string): Promise<OpenCamp> {
    return invoke("camp_join", { link, name });
  }

  open(id: string): Promise<OpenCamp> {
    return invoke("camp_open", { id });
  }

  leave(id: string): Promise<boolean> {
    return invoke<boolean>("camp_leave", { id });
  }

  invite(id: string): Promise<string> {
    return invoke("camp_invite", { id });
  }

  onLive(listener: (live: CampLive) => void): () => void {
    return subscribe<CampLive>(CAMP_STATE_EVENT, listener);
  }
}
