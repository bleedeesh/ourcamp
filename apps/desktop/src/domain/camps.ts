import type { CampApi, CampDirective, CampLive, CampPreview, CampSummary, OpenCamp } from "../engine/camp-api.ts";
import { createStore, type Store } from "../lib/store.ts";
import { avatarColor, campShort, campfiresOf } from "./roster.ts";
import type { Camp } from "./types.ts";
import type { VoiceController } from "./voice.ts";

/** What the main window shows. */
export type CampsView = "loading" | "empty" | "join" | "camp";

/** State of the link typed into the join form. */
export type LinkCheck =
  | { readonly state: "idle" }
  | { readonly state: "checking" }
  | { readonly state: "ok"; readonly preview: CampPreview }
  | { readonly state: "error"; readonly message: string };

export interface CampsState {
  readonly view: CampsView;
  /** Name the user goes by in camps (until accounts, M4). */
  readonly displayName: string;
  readonly camps: readonly CampSummary[];
  readonly currentId: string | null;
  /** The open camp's server is unreachable; its channels are the last known ones. */
  readonly offline: boolean;
  readonly switcherOpen: boolean;
  /** The camp menu asks "leave this camp?" instead of listing camps. */
  readonly leaveAsk: boolean;
  // Join form
  readonly link: string;
  readonly name: string;
  readonly check: LinkCheck;
  readonly joining: boolean;
  readonly joinError: string | null;
  /** Short confirmation ("link copied"). */
  readonly notice: string | null;
  /** Your avatar as this device keeps it (ADR-0035): offered in the join form. */
  readonly avatar: Uint8Array | null;
  readonly error: string | null;
}

/** Placeholder until a camp is open: the camp view is not rendered then. */
export const NO_CAMP: Camp = { id: "", name: "", short: "", color: "#444444", channels: [], members: {} };

/** `ourcamp://host[:port]/join|claim/<token>?…fp=<43>` — enough to ask the camp about it. */
const LINK_SHAPE = /^ourcamp:\/\/[^/\s]+\/(join|claim)\/[0-9a-z]+\?\S*fp=[A-Za-z0-9_-]{43}/i;

// The badge is recomputed on live renames too (roster.ts), so it lives there.
export { campShort };

/** The camp as the channel tree needs it: campfires only — chats are not in the tree. */
export function campFromOpen(open: OpenCamp): Camp {
  return {
    id: open.id,
    name: open.name,
    short: campShort(open.name),
    color: avatarColor(open.id),
    channels: campfiresOf(open.channels, []),
    members: {
      [open.self.userId]: {
        id: open.self.userId,
        name: open.self.name,
        color: avatarColor(open.self.userId),
        micMuted: false,
        deafened: false,
        silenced: false,
        role: open.self.role,
        streaming: false,
  avatar: null,
      },
    },
  };
}

export interface CampsOptions {
  /** Puts text on the clipboard (the webview's in the app, a fake in tests). */
  readonly copy: (text: string) => Promise<void>;
  /** Builds the tree's camp from an opened one (the browser demo substitutes its people). */
  readonly toCamp?: (open: OpenCamp) => Camp;
}

/**
 * Which camp is open and how you get into one: the first-run screen, the join form
 * (paste a link, see where it leads, choose a name), the switcher and "copy invite".
 * Voice inside the open camp stays with `VoiceController`.
 */
export class CampsController {
  readonly store: Store<CampsState>;
  readonly #api: CampApi;
  readonly #voice: VoiceController;
  readonly #copy: (text: string) => Promise<void>;
  readonly #toCamp: (open: OpenCamp) => Camp;
  /** Bumped per link check; answers to older checks are dropped. */
  #checkSeq = 0;
  #unsubscribeLive: (() => void) | null = null;
  #unsubscribeDirective: (() => void) | null = null;

  constructor(api: CampApi, voice: VoiceController, options: CampsOptions) {
    this.#api = api;
    this.#voice = voice;
    this.#copy = options.copy;
    this.#toCamp = options.toCamp ?? campFromOpen;
    this.store = createStore<CampsState>({
      view: "loading",
      displayName: "",
      camps: [],
      currentId: null,
      offline: false,
      switcherOpen: false,
      leaveAsk: false,
      link: "",
      name: "",
      check: { state: "idle" },
      joining: false,
      joinError: null,
      notice: null,
      avatar: null,
      error: null,
    });
  }

  /** Loads the camp list and opens the last camp (or shows the first-run screen). */
  /** Avatars by hash: fetched once per run (the app keeps them on disk as well). */
  readonly #avatars = new Map<string, Promise<Uint8Array>>();

  /**
   * A member's avatar of the open camp by the hash its snapshot names. A failure is not
   * remembered: the next render asks again.
   */
  avatarOf(userId: string, hash: string): Promise<Uint8Array> {
    const { currentId } = this.store.get();
    if (currentId === null) return Promise.reject(new Error("no camp open"));
    let picture = this.#avatars.get(hash);
    if (!picture) {
      picture = this.#api.avatar(currentId, userId, hash);
      this.#avatars.set(hash, picture);
      picture.catch(() => this.#avatars.delete(hash));
    }
    return picture;
  }

  /** Sets your avatar (bytes from `makeAvatar`) or drops it (ADR-0035). */
  async setAvatar(picture: Uint8Array | null): Promise<void> {
    try {
      await this.#api.setProfileAvatar(picture);
      this.store.patch({ avatar: picture });
    } catch (err) {
      this.store.patch({ error: message(err) });
    }
  }

  async start(): Promise<void> {
    this.#unsubscribeLive ??= this.#api.onLive((live) => this.#onLive(live));
    this.#unsubscribeDirective ??= this.#api.onDirective((d) => void this.#onDirective(d));
    void this.#api.profileAvatar().then(
      (avatar) => this.store.patch({ avatar }),
      () => {},
    );
    try {
      const list = await this.#api.list();
      this.store.patch({ displayName: list.displayName, camps: list.camps, name: list.displayName });
      const id = list.current ?? list.camps[0]?.id ?? null;
      if (id === null) this.store.patch({ view: "empty" });
      else await this.#open(id);
    } catch (err) {
      this.store.patch({ view: "empty", error: message(err) });
    }
  }

  // ---- join form ----

  openJoin(): void {
    const { displayName } = this.store.get();
    this.store.patch({
      view: "join",
      switcherOpen: false,
      link: "",
      name: displayName,
      check: { state: "idle" },
      joinError: null,
    });
  }

  cancelJoin(): void {
    this.store.patch({ view: this.store.get().currentId === null ? "empty" : "camp" });
  }

  /** Updates the link and, once it looks like an invite, asks the camp about it. */
  async setLink(link: string): Promise<void> {
    const seq = ++this.#checkSeq;
    const trimmed = link.trim();
    this.store.patch({ link, joinError: null });
    if (trimmed === "") return this.store.patch({ check: { state: "idle" } });
    if (!trimmed.toLowerCase().startsWith("ourcamp://")) {
      return this.store.patch({ check: { state: "error", message: "Это не ссылка OurCamp — она начинается с ourcamp://" } });
    }
    if (!LINK_SHAPE.test(trimmed)) {
      // Still being typed or cut short; say nothing until it could be complete.
      return this.store.patch({ check: { state: "idle" } });
    }
    this.store.patch({ check: { state: "checking" } });
    try {
      const preview = await this.#api.preview(trimmed);
      if (seq === this.#checkSeq) this.store.patch({ check: { state: "ok", preview } });
    } catch (err) {
      if (seq === this.#checkSeq) this.store.patch({ check: { state: "error", message: message(err) } });
    }
  }

  setName(name: string): void {
    this.store.patch({ name, joinError: null });
  }

  /** Whether "Войти на сервер" can be pressed. */
  canJoin(): boolean {
    const { check, name, joining } = this.store.get();
    return !joining && check.state === "ok" && check.preview.usable && name.trim() !== "";
  }

  async join(): Promise<void> {
    if (!this.canJoin()) return;
    const { link, name } = this.store.get();
    this.store.patch({ joining: true, joinError: null });
    try {
      const opened = await this.#api.join(link.trim(), name.trim());
      const list = await this.#api.list();
      this.store.patch({ camps: list.camps, displayName: list.displayName, joining: false });
      await this.#show(opened);
    } catch (err) {
      this.store.patch({ joining: false, joinError: message(err) });
    }
  }

  // ---- switcher ----

  toggleSwitcher(): void {
    this.store.patch({ switcherOpen: !this.store.get().switcherOpen, leaveAsk: false });
  }

  closeSwitcher(): void {
    this.store.patch({ switcherOpen: false, leaveAsk: false });
  }

  /** "Покинуть сервер" in the menu: ask first — coming back takes a new invite. */
  askLeave(): void {
    this.store.patch({ leaveAsk: true });
  }

  cancelLeave(): void {
    this.store.patch({ leaveAsk: false });
  }

  /**
   * Leaves the open camp: its voice, then the camp itself; opens the next camp in the
   * list, or the first-run screen when none is left.
   */
  async leave(): Promise<void> {
    const { currentId, camps } = this.store.get();
    this.store.patch({ switcherOpen: false, leaveAsk: false });
    if (currentId === null) return;
    const name = camps.find((c) => c.id === currentId)?.name ?? "";
    await this.#voice.leave();
    try {
      const told = await this.#api.leave(currentId);
      const list = await this.#api.list();
      this.store.patch({ camps: list.camps, currentId: null, offline: false });
      const next = list.camps[0]?.id;
      if (next === undefined) this.store.patch({ view: "empty" });
      else await this.#open(next);
      this.store.patch({
        notice: told
          ? `Вы покинули сервер «${name}»`
          : `Сервер «${name}» убран из списка. Он сейчас недоступен, поэтому там вы пока числитесь участником`,
      });
    } catch (err) {
      this.store.patch({ error: message(err) });
    }
  }

  async switchTo(id: string): Promise<void> {
    this.store.patch({ switcherOpen: false });
    if (id === this.store.get().currentId) return;
    await this.#open(id);
  }

  async copyInvite(): Promise<void> {
    const { currentId } = this.store.get();
    this.store.patch({ switcherOpen: false });
    if (currentId === null) return;
    try {
      await this.#copy(await this.#api.invite(currentId));
      this.store.patch({ notice: "Ссылка-приглашение скопирована — отправьте её друзьям" });
    } catch (err) {
      this.store.patch({ error: message(err) });
    }
  }

  /**
   * Someone dragged onto another campfire (ADR-0029): yourself — you join it; anyone
   * else — the camp tells their app to (rights are checked there too).
   */
  async moveMember(userId: string, channelId: string): Promise<void> {
    if (userId === this.#voice.store.get().selfId) return this.#voice.join(channelId);
    const { currentId } = this.store.get();
    if (currentId === null) return;
    try {
      await this.#api.moveMember(currentId, userId, channelId);
    } catch (err) {
      this.store.patch({ error: message(err) });
    }
  }

  /**
   * Kicks someone of a lower rank (ADR-0033). The camp's snapshot then drops them from
   * the tree; here we only say it happened.
   */
  async kick(userId: string): Promise<void> {
    const { currentId } = this.store.get();
    if (currentId === null) return;
    const name = this.#voice.store.get().camp.members[userId]?.name ?? "";
    try {
      await this.#api.kick(currentId, userId);
      this.store.patch({ notice: `«${name}» выгнан с сервера. Вернуться можно по приглашению` });
    } catch (err) {
      this.store.patch({ error: message(err) });
    }
  }

  /** Forbids someone of a lower rank to speak, or lifts it (ADR-0033); the snapshot shows it. */
  async setSilenced(userId: string, silenced: boolean): Promise<void> {
    const { currentId } = this.store.get();
    if (currentId === null) return;
    try {
      await this.#api.setSilenced(currentId, userId, silenced);
    } catch (err) {
      this.store.patch({ error: message(err) });
    }
  }

  /** Live state of the open camp (ADR-0021); updates for a camp we already left are dropped. */
  #onLive(live: CampLive): void {
    if (live.campId !== this.store.get().currentId) return;
    this.store.patch({ offline: !live.online });
    if (!live.state) return;
    this.#voice.applyCampState(live.state);
    // A rename shows in the switcher too, not only in the header.
    const { name } = live.state;
    const camps = this.store.get().camps;
    if (camps.some((c) => c.id === live.campId && c.name !== name)) {
      this.store.patch({ camps: camps.map((c) => (c.id === live.campId ? { ...c, name } : c)) });
    }
  }

  /** A moderator moved or kicked us (ADR-0028). */
  async #onDirective(directive: CampDirective): Promise<void> {
    if (directive.type === "moved") {
      if (directive.campId !== this.store.get().currentId) return;
      // Only someone in voice is moved; if we left meanwhile, stay out.
      if (this.#voice.store.get().channelId === null) return;
      await this.#voice.join(directive.channelId).catch(() => {});
      const channel = this.#voice.store.get().camp.channels.find((c) => c.id === directive.channelId);
      if (channel) this.store.patch({ notice: `Вас перенесли в канал «${channel.name}»` });
      return;
    }
    const wasOpen = directive.campId === this.store.get().currentId;
    if (wasOpen) {
      await this.#voice.leave();
      // LiveKit put us out of the room too; the kick notice says why, not "removed".
      this.#voice.dismissError();
    }
    try {
      const list = await this.#api.list();
      this.store.patch({ camps: list.camps, switcherOpen: false, leaveAsk: false });
      if (wasOpen) {
        this.store.patch({ currentId: null, offline: false });
        const next = list.camps[0]?.id;
        if (next === undefined) this.store.patch({ view: "empty" });
        else await this.#open(next);
      }
      this.store.patch({ notice: `Вас выгнали с сервера «${directive.campName}». Вернуться можно по приглашению` });
    } catch (err) {
      this.store.patch({ error: message(err) });
    }
  }

  dismissNotice(): void {
    this.store.patch({ notice: null });
  }

  dismissError(): void {
    this.store.patch({ error: null });
  }

  async #open(id: string): Promise<void> {
    try {
      await this.#show(await this.#api.open(id));
    } catch (err) {
      this.store.patch({ error: message(err), view: this.store.get().currentId === null ? "empty" : "camp" });
    }
  }

  async #show(opened: OpenCamp): Promise<void> {
    await this.#voice.openCamp(this.#toCamp(opened), opened.self.userId);
    this.store.patch({
      view: "camp",
      currentId: opened.id,
      offline: opened.offline,
      error: opened.offline ? "Сервер сейчас недоступен — показаны последние известные каналы" : null,
    });
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** "3 канала", "5 каналов", "1 канал". */
export function campfires(n: number): string {
  const mod10 = n % 10;
  const mod100 = n % 100;
  if (mod10 === 1 && mod100 !== 11) return `${n} канал`;
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) return `${n} канала`;
  return `${n} каналов`;
}
