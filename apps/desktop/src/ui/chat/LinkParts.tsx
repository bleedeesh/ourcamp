import type { ReactNode } from "react";
import type { LinkCard } from "../../engine/camp-api.ts";
import { siteOf, splitLinks } from "../../domain/links.ts";
import { useChatController } from "./chat-context.tsx";

/** A link that opens in the system's browser, never in this window (ADR-0077). */
function ExternalLink({ href, className, children }: { href: string; className: string; children: ReactNode }) {
  const chat = useChatController();
  return (
    <a
      className={className}
      href={href}
      title={href}
      draggable={false}
      onClick={(e) => {
        e.preventDefault();
        void chat.openLink(href);
      }}
      // A middle click would open a window of the webview.
      onAuxClick={(e) => e.preventDefault()}
    >
      {children}
    </a>
  );
}

/** A message's text, its links clickable. */
export function MessageText({ text }: { text: string }) {
  return (
    <p className="message__text">
      {splitLinks(text).map((part, i) =>
        part.href === undefined ? (
          part.text
        ) : (
          <ExternalLink key={i} className="message__link" href={part.href}>
            {part.text}
          </ExternalLink>
        ),
      )}
    </p>
  );
}

/** A link's card under a message; `picture` is drawn by the feed, like any photo. */
export function LinkCardView({ card, picture }: { card: LinkCard; picture: ReactNode }) {
  const href = splitLinks(card.url)[0]?.href;
  if (href === undefined) return null;
  const site = siteOf(card.url);
  return (
    <div className="link-card">
      <ExternalLink className="link-card__site" href={href}>
        {site}
      </ExternalLink>
      {card.title !== "" && (
        <ExternalLink className="link-card__title" href={href}>
          {card.title}
        </ExternalLink>
      )}
      {card.description !== undefined && card.description !== "" && (
        <p className="link-card__description">{card.description}</p>
      )}
      {picture}
    </div>
  );
}
