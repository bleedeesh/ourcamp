// «Адрес для друзей» (ADR-0037): mirrors `src-tauri/src/address.rs`; the words the
// window says about each choice.

export type AddressKind = "lan" | "public" | "radmin" | "hamachi" | "tailscale" | "router";

export type AddressChoice =
  | { kind: "auto" }
  | { kind: "detected"; which: AddressKind }
  | { kind: "custom"; host: string };

export interface AddressOption {
  kind: AddressKind;
  ip: string;
  /** The router's address is "grey" (CGNAT): no way in from the internet. */
  grey: boolean;
}

/** The camp's ports on the router (ADR-0039). */
export type Ports =
  | { state: "opening" }
  | { state: "open"; externalIp: string; answers: boolean | null }
  | { state: "failed"; message: string };

export interface Reach {
  choice: AddressChoice;
  /** Detected now: the main network first, then each VPN found. */
  options: AddressOption[];
  /** Why the link is not at the chosen address right now. */
  note: string | null;
  /** The ports on the router; null unless the internet address is chosen. */
  ports: Ports | null;
}

export const NO_REACH: Reach = { choice: { kind: "auto" }, options: [], note: null, ports: null };

export const KIND_TITLE: Record<AddressKind, string> = {
  lan: "Локальная сеть",
  public: "Внешний адрес",
  radmin: "Radmin VPN",
  hamachi: "Hamachi",
  tailscale: "Tailscale",
  router: "Интернет через роутер",
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
    case "router":
      return "Друзья заходят из интернета. Порты на роутере сервер откроет сам (UPnP) и закроет, когда остановится.";
  }
}

/** The hint under the choice; `null` when a warning says it all (a grey router address). */
export function addressHint(reach: Reach): string | null {
  const { choice, options } = reach;
  switch (choice.kind) {
    case "auto":
      return needs(options[0]?.kind ?? "lan");
    case "detected":
      if (options.some((o) => o.kind === choice.which && o.grey)) return null;
      return needs(choice.which);
    case "custom":
      return `Домен или IP, по которому вас видно из интернета: DDNS, статичный IP, хостинг. ${PORTS}`;
  }
}

/** The line about the router's ports, and whether it is a warning. */
export function portsLine(ports: Ports | null): { text: string; warn: boolean } | null {
  switch (ports?.state) {
    case undefined:
      return null;
    case "opening":
      return { text: "Открываем порты на роутере…", warn: false };
    case "open":
      return ports.answers
        ? { text: `Порты открыты на роутере, сервер отвечает по ${ports.externalIp}.`, warn: false }
        : {
            text: "Порты открыты на роутере. Изнутри сети это не проверить — роутер не возвращает подключения к себе. Попросите друга зайти по ссылке.",
            warn: false,
          };
    case "failed":
      return { text: `Не получилось открыть порты: ${ports.message}. Откройте 7780 и 7881 (TCP), 7882 (UDP) в настройках роутера вручную.`, warn: true };
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
  for (const o of reach.options) {
    entries.push({ value: `detected:${o.kind}`, label: KIND_TITLE[o.kind], detail: o.grey ? `${o.ip} · серый` : o.ip });
  }
  const { choice } = reach;
  if (choice.kind === "detected" && !reach.options.some((o) => o.kind === choice.which)) {
    entries.push({ value: choiceValue(choice), label: KIND_TITLE[choice.which] ?? choice.which, detail: "не найден" });
  }
  entries.push({ value: "custom", label: "Свой адрес: домен или IP" });
  return entries;
}
