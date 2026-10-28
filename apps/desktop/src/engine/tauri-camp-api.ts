import { invoke } from "@tauri-apps/api/core";
import type { CampApi, CampList, CampPreview, OpenCamp } from "./camp-api.ts";

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

  forget(id: string): Promise<void> {
    return invoke("camp_forget", { id });
  }

  invite(id: string): Promise<string> {
    return invoke("camp_invite", { id });
  }
}
