import { Avatar } from "./ChannelTree.tsx";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";

export function BottomPanel() {
  const voice = useVoiceController();
  const channelName = useVoice((s) => s.camp.channels.find((c) => c.id === s.channelId)?.name ?? null);
  const self = useVoice((s) => s.camp.members[s.selfId]);
  const speaking = useVoice((s) => s.speaking[s.selfId] === true);
  const micMuted = useVoice((s) => s.micMuted);
  const deafened = useVoice((s) => s.deafened);

  return (
    <section className="panel" aria-label="Голос и управление">
      {channelName !== null ? (
        <div className="panel__connection">
          <span className="dot dot--online" aria-hidden />
          <span className="panel__connection-text">
            В канале <b>{channelName}</b>
          </span>
          <button type="button" className="square-button square-button--danger" aria-label="Выйти из голосового канала" onClick={() => void voice.leave()}>
            <Icon name="leave" size={16} />
          </button>
        </div>
      ) : (
        <p className="panel__hint">Не в канале — выберите канал выше</p>
      )}

      <div className="panel__self">
        {self && <Avatar member={self} size={32} speaking={speaking && !micMuted} />}
        <span className="panel__who">
          <span className="panel__name">{self?.name}</span>
          <span className="panel__status">{channelName !== null ? "В канале" : "В сети"}</span>
        </span>
        <button
          type="button"
          className="square-button"
          data-off={micMuted || undefined}
          aria-pressed={micMuted}
          aria-label={micMuted ? "Включить микрофон" : "Выключить микрофон"}
          onClick={() => void voice.toggleMic()}
        >
          <Icon name="mic" off={micMuted} />
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
