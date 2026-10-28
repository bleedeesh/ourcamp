import { linkView, type LinkView } from "../domain/link.ts";
import { Avatar } from "./ChannelTree.tsx";
import { useCamps } from "./camps-context.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";

/** Signal bars and the ping (ADR-0030); quality and channel are in the tooltip. */
function Connection({ view, channel }: { view: LinkView; channel: string }) {
  return (
    <span
      className="link-status"
      data-tone={view.tone}
      role="status"
      title={`${view.quality} · канал «${channel}»`}
      aria-label={`${view.quality}, ${view.text}`}
    >
      <svg className="link-status__bars" width="16" height="14" viewBox="0 0 16 14" aria-hidden="true">
        {[0, 1, 2].map((i) => (
          <rect key={i} x={i * 6} y={9 - i * 4.5} width="4" height={5 + i * 4.5} rx="1.2" data-on={i < view.bars || undefined} />
        ))}
      </svg>
      <span className="link-status__ping">{view.text}</span>
    </span>
  );
}

export function BottomPanel() {
  const voice = useVoiceController();
  const channelName = useVoice((s) => s.camp.channels.find((c) => c.id === s.channelId)?.name ?? null);
  const self = useVoice((s) => s.camp.members[s.selfId]);
  const speaking = useVoice((s) => s.speaking[s.selfId] === true);
  const micMuted = useVoice((s) => s.micMuted);
  const deafened = useVoice((s) => s.deafened);
  const link = useVoice((s) => s.link);
  const campOffline = useCamps((s) => s.offline);
  // A moderator forbade speaking (ADR-0028): the button is locked until lifted.
  const silenced = self?.silenced === true;
  const micOff = micMuted || silenced;

  return (
    <section className="panel" aria-label="Голос и управление">
      {channelName !== null ? (
        <div className="panel__connection">
          <Connection view={linkView(link, !campOffline)} channel={channelName} />
          <button type="button" className="square-button square-button--danger" aria-label="Выйти из голосового канала" onClick={() => void voice.leave()}>
            <Icon name="leave" size={16} />
          </button>
        </div>
      ) : (
        <p className="panel__hint">Не в канале — выберите канал выше</p>
      )}

      <div className="panel__self">
        {self && <Avatar member={self} size={32} speaking={speaking && !micOff} />}
        <span className="panel__who">
          <span className="panel__name">{self?.name}</span>
          <span className="panel__status" data-warning={silenced || undefined}>
            {silenced ? "Говорить запрещено" : channelName !== null ? "В канале" : "В сети"}
          </span>
        </span>
        <button
          type="button"
          className="square-button"
          data-off={micOff || undefined}
          aria-pressed={micOff}
          aria-disabled={silenced || undefined}
          aria-label={silenced ? "Микрофон: модератор запретил вам говорить" : micMuted ? "Включить микрофон" : "Выключить микрофон"}
          title={silenced ? "Модератор запретил вам говорить" : undefined}
          onClick={() => void voice.toggleMic()}
        >
          <Icon name="mic" off={micOff} />
        </button>
        <button
          type="button"
          className="square-button"
          data-off={deafened || undefined}
          aria-pressed={deafened}
          aria-label={deafened ? "Включить звук" : "Выключить звук"}
          onClick={() => void voice.toggleDeafen()}
        >
          <Icon name="headphones" off={deafened} />
        </button>
        <button type="button" className="square-button" aria-label="Настройки" aria-disabled="true" title="Скоро">
          <Icon name="gear" />
        </button>
      </div>

      {/* Chat and streams open their own windows (roadmap M5/M6). */}
      <div className="panel__windows">
        <button type="button" className="wide-button" aria-disabled="true" title="Скоро">
          <Icon name="chat" size={18} />
          Чат
        </button>
        <button type="button" className="wide-button" aria-disabled="true" title="Скоро">
          <Icon name="screen" size={18} />
          Стрим
        </button>
      </div>
    </section>
  );
}
