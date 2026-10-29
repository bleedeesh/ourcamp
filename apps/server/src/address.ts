// «Адрес для друзей» (ADR-0037): mirrors `src-tauri/src/address.rs`; the words the
// window says about each choice.

export type AddressKind = "lan" | "public" | "radmin" | "hamachi" | "tailscale";

export type AddressChoice =
  | { kind: "auto" }
  | { kind: "detected"; which: AddressKind }
  | { kind: "custom"; host: string };

export interface AddressOption {
  kind: AddressKind;
  ip: string;
}

export interface Reach {
  choice: AddressChoice;
  /** Detected now: the main network first, then each VPN found. */
  options: AddressOption[];
  /** Why the link is not at the chosen address right now. */
  note: string | null;
}

export const NO_REACH: Reach = { choice: { kind: "auto" }, options: [], note: null };

export const KIND_TITLE: Record<AddressKind, string> = {
  lan: "Локальная сеть",
  public: "Внешний адрес",
  radmin: "Radmin VPN",
  hamachi: "Hamachi",
  tailscale: "Tailscale",
};

const PORTS = "Порты 7780 и 7881 (TCP) и 7882 (UDP) должны быть открыты.";

/** What friends need for a kind of address. */
function needs(kind: AddressKind): string {
  switch (kind) {
    case "lan":
      return "Зайдут только те, кто в той же сети: один роутер или Wi-Fi.";
    case "public":
      return `Друзья заходят из интернета. ${PORTS}`;
    case "radmin":
    case "hamachi":
    case "tailscale":
      return `Друзья должны быть в той же сети ${KIND_TITLE[kind]}. Порты открывать не нужно.`;
  }
}

/** The hint under the choice. */
export function addressHint(reach: Reach): string {
  const { choice, options } = reach;
  switch (choice.kind) {
    case "auto":
      return needs(options[0]?.kind ?? "lan");
    case "detected":
      return needs(choice.which);
    case "custom":
      return `Домен или IP, по которому вас видно из интернета: DDNS, статичный IP, хостинг. ${PORTS}`;
  }
}

/** A choice as a <select> value, and back. */
export function choiceValue(choice: AddressChoice): string {
  switch (choice.kind) {
    case "auto":
      return "auto";
    case "detected":
      return `detected:${choice.which}`;
    case "custom":
      return "custom";
  }
}

export function choiceFromValue(value: string, host: string): AddressChoice {
  if (value === "custom") return { kind: "custom", host };
  if (value.startsWith("detected:")) return { kind: "detected", which: value.slice("detected:".length) as AddressKind };
  return { kind: "auto" };
}

/** One entry of the list: the address's name, its IP muted on the right. */
export interface AddressEntry {
  value: string;
  label: string;
  detail?: string;
}

/** The list's entries: automatic, each detected network (and a kept one that is gone), custom. */
export function addressEntries(reach: Reach): AddressEntry[] {
  const [main] = reach.options;
  const entries: AddressEntry[] = [main ? { value: "auto", label: "Автоматически", detail: main.ip } : { value: "auto", label: "Автоматически" }];
  for (const o of reach.options) entries.push({ value: `detected:${o.kind}`, label: KIND_TITLE[o.kind], detail: o.ip });
  const { choice } = reach;
  if (choice.kind === "detected" && !reach.options.some((o) => o.kind === choice.which)) {
    entries.push({ value: choiceValue(choice), label: KIND_TITLE[choice.which] ?? choice.which, detail: "не найден" });
  }
  entries.push({ value: "custom", label: "Свой адрес: домен или IP" });
  return entries;
}
