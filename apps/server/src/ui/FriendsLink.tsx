import { useEffect, useState, type FormEvent } from "react";
import { copyText } from "@ourcamp/ui/clipboard.ts";
import { Select } from "@ourcamp/ui/Select.tsx";

import { addressEntries, addressHint, choiceFromValue, choiceValue, type AddressChoice, type Reach } from "../address.ts";
import type { ServerApi } from "../api.ts";
import { useAction } from "./CampSettings.tsx";

/**
 * «Для друзей»: the link to send, and the address it carries (ADR-0037) — the local
 * network, a VPN like Radmin, the public address of a hosting server, or one's own
 * domain or IP. A new address restarts the camp: voice drops for a moment.
 */
export function FriendsLink({ link, reach, api }: { link: string; reach: Reach; api: ServerApi }) {
  const [copied, setCopied] = useState<"idle" | "copied" | string>("idle");
  const [value, setValue] = useState(choiceValue(reach.choice));
  const [host, setHost] = useState(reach.choice.kind === "custom" ? reach.choice.host : "");
  const [error, run, clear] = useAction();

  // A restart or another window's change brings a new choice: follow it.
  useEffect(() => {
    setValue(choiceValue(reach.choice));
    if (reach.choice.kind === "custom") setHost(reach.choice.host);
  }, [reach.choice]);

  useEffect(() => {
    if (copied !== "copied") return;
    const timer = setTimeout(() => setCopied("idle"), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  const copy = () => {
    copyText(link).then(
      () => setCopied("copied"),
      (e: unknown) => setCopied(e instanceof Error ? e.message : String(e)),
    );
  };

  const apply = (choice: AddressChoice) => void run(() => api.setAddress(choice));

  const pick = (next: string) => {
    clear();
    setValue(next);
    // One's own address is typed first; the others apply at once.
    if (next !== "custom") apply(choiceFromValue(next, ""));
  };

  const saveHost = (e: FormEvent) => {
    e.preventDefault();
    apply({ kind: "custom", host: host.trim() });
  };

  const shown: Reach = { ...reach, choice: choiceFromValue(value, host) };
  const editingHost = value === "custom";
  const hostChanged = reach.choice.kind !== "custom" || reach.choice.host !== host.trim();

  return (
    <section className="link">
      <div className="link__head">
        <span className="link__label">Для друзей</span>
        <button type="button" className="link__copy" onClick={copy}>
          {copied === "copied" ? "Скопировано" : "Скопировать"}
        </button>
      </div>
      <code className="link__value">{link}</code>
      <span className="link__hint">{copied !== "idle" && copied !== "copied" ? copied : "Отправьте её тем, кого зовёте на сервер."}</span>

      <div className="address">
        <span id="friends-address-label" className="address__label">
          Адрес в ссылке
        </span>
        <Select
          id="friends-address"
          labelledBy="friends-address-label"
          value={value}
          options={addressEntries(reach)}
          // Networks come and go (a VPN switched on): look again when the list opens.
          onOpen={() => void api.refreshAddresses()}
          onChange={pick}
        />
        {editingHost && (
          <form className="address__custom" onSubmit={saveHost}>
            <input
              className="name-edit__input"
              placeholder="mycamp.duckdns.org или 85.12.34.56"
              aria-label="Свой адрес: домен или IP"
              value={host}
              spellCheck={false}
              onChange={(e) => {
                clear();
                setHost(e.target.value);
              }}
            />
            <button type="submit" className="small-button small-button--accent" disabled={host.trim() === "" || !hostChanged}>
              Применить
            </button>
          </form>
        )}
        <span className="address__hint">{addressHint(shown)}</span>
        {reach.note && <span className="address__note">{reach.note}</span>}
        {error && <span className="name-edit__error">{error}</span>}
        <span className="address__hint">Смена адреса перезапускает сервер: разговор прервётся на пару секунд.</span>
      </div>
    </section>
  );
}
