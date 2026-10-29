import { invoke } from "@tauri-apps/api/core";
import type { CampApi, CampDirective, CampList, CampLive, CampPreview, OpenCamp } from "./camp-api.ts";
import { subscribe } from "@ourcamp/ui/tauri-events.ts";
import { decodeBase64Url, encodeBase64Url } from "../domain/avatar.ts";

/** Event emitted by `src-tauri/src/lib.rs` (`CAMP_STATE_EVENT`). */
const CAMP_STATE_EVENT = "camp://state";
/** Event emitted by `src-tauri/src/lib.rs` (`CAMP_DIRECTIVE_EVENT`). */
const CAMP_DIRECTIVE_EVENT = "camp://directive";

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

  moveMember(id: string, userId: string, channelId: string): Promise<void> {
    return invoke("camp_move_member", { id, userId, channelId });
  }

  async profileAvatar(): Promise<Uint8Array | null> {
    const picture = await invoke<string | null>("profile_avatar");
    return picture === null ? null : decodeBase64Url(picture);
  }

  setProfileAvatar(picture: Uint8Array | null): Promise<void> {
    return invoke("profile_set_avatar", { picture: picture === null ? null : encodeBase64Url(picture) });
  }

  async avatar(id: string, userId: string, hash: string): Promise<Uint8Array> {
    return decodeBase64Url(await invoke<string>("camp_avatar", { id, userId, hash }));
  }

  kick(id: string, userId: string): Promise<void> {
    return invoke("camp_kick", { id, userId });
  }

  setSilenced(id: string, userId: string, silenced: boolean): Promise<void> {
    return invoke("camp_set_silenced", { id, userId, silenced });
  }

  onLive(listener: (live: CampLive) => void): () => void {
    return subscribe<CampLive>(CAMP_STATE_EVENT, listener);
  }

  onDirective(listener: (directive: CampDirective) => void): () => void {
    return subscribe<CampDirective>(CAMP_DIRECTIVE_EVENT, listener);
  }
}
