import type { MouseEvent, PointerEvent } from "react";
import { canMove } from "../domain/roster.ts";
import type { Channel, UserId } from "../domain/types.ts";
import { DEFAULT_VOLUME } from "../domain/volume.ts";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { DROP_ATTRIBUTE, dropTarget } from "@ourcamp/ui/drag.ts";
import { Icon } from "@ourcamp/ui/Icon.tsx";
import { DragGhost, useDragMove, type Drag, type Draggable } from "@ourcamp/ui/useDragMove.tsx";
import { Avatar } from "./Avatar.tsx";
import { useCampsController } from "./camps-context.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";

/** Dragging people between channels (ADR-0029). */
interface Dragging {
  readonly drag: Drag | null;
  readonly grab: (who: Draggable) => (e: PointerEvent<HTMLElement>) => void;
}

export function ChannelTree() {
  const channels = useVoice((s) => s.camp.channels);
  const camps = useCampsController();
  const { drag, grab } = useDragMove((userId, channelId) => void camps.moveMember(userId, channelId));
  return (
    <nav className="tree" aria-label="Голосовые каналы">
      {channels.map((channel) => (
        <ChannelNode key={channel.id} channel={channel} drag={drag} grab={grab} />
      ))}
      <DragGhost drag={drag} />
    </nav>
  );
}

function ChannelNode({ channel, drag, grab }: { channel: Channel } & Dragging) {
  const voice = useVoiceController();
  const current = useVoice((s) => s.channelId === channel.id);
  const selfId = useVoice((s) => s.selfId);
  const memberIds = current ? [...channel.memberIds, selfId] : channel.memberIds;
  const lit = memberIds.length > 0;

  return (
    <div
      className="channel"
      {...{ [DROP_ATTRIBUTE]: channel.id }}
      data-drop-target={(drag !== null && dropTarget(drag.from, drag.over) === channel.id) || undefined}
    >
      <button
        type="button"
        className="channel__row"
        aria-current={current ? "true" : undefined}
        title={channel.afk ? "Отошёл: микрофон и звук выключаются, пока вы здесь" : undefined}
        onClick={() => void voice.join(channel.id)}
      >
        {channel.afk ? (
          <Icon name="moon" size={20} className="channel__afk" />
        ) : (
          <Campfire size={20} simple={!lit} dim={!lit} />
        )}
        <span className="channel__name">{channel.name}</span>
        {lit && <span className="channel__count">{memberIds.length}</span>}
      </button>
      {lit && (
        <ul className="channel__members">
          {memberIds.map((id) => (
            <MemberRow key={id} userId={id} channelId={channel.id} drag={drag} grab={grab} />
          ))}
        </ul>
      )}
    </div>
  );
}

function MemberRow({ userId, channelId, drag, grab }: { userId: UserId; channelId: string } & Dragging) {
  const voice = useVoiceController();
  const member = useVoice((s) => s.camp.members[userId]);
  const isSelf = useVoice((s) => s.selfId === userId);
  const speaking = useVoice((s) => s.speaking[userId] === true);
  const selfMicMuted = useVoice((s) => s.micMuted);
  const selfDeafened = useVoice((s) => s.deafened);
  const prefs = useVoice((s) => s.prefs[userId]);
  // Yourself — to change channel; others — with a moderator's rights (ADR-0029).
  const movable = useVoice((s) => canMove(s.camp, s.selfId, userId));

  if (!member) return null;
  const micMuted = isSelf ? selfMicMuted : member.micMuted;
  const deafened = isSelf ? selfDeafened : member.deafened;
  const localMuted = prefs?.muted === true;
  const volume = prefs?.volume ?? DEFAULT_VOLUME;

  const openMenu = (e: MouseEvent<HTMLElement>) => {
    if (isSelf) return;
    e.preventDefault();
    const rect = e.currentTarget.getBoundingClientRect();
    // Keyboard activation has no pointer position; anchor under the row instead.
    const fromPointer = e.clientX !== 0 || e.clientY !== 0;
    voice.openVolumeMenu({
      userId,
      x: fromPointer ? e.clientX : rect.left + 24,
      y: fromPointer ? e.clientY : rect.bottom,
    });
  };

  return (
    <li>
      <button
        type="button"
        className="member"
        data-self={isSelf || undefined}
        data-draggable={movable || undefined}
        data-dragging={drag?.userId === userId || undefined}
        onPointerDown={movable ? grab({ userId, name: member.name, from: channelId }) : undefined}
        // The person's menu (volume, moderation) is a right click, as context menus are;
        // the left button is for dragging. Enter or Space on the row opens it too: a
        // keyboard click carries no mouse presses (`detail` 0).
        onClick={(e) => {
          if (e.detail === 0) openMenu(e);
        }}
        onContextMenu={openMenu}
        aria-haspopup={isSelf ? undefined : "dialog"}
        aria-label={isSelf ? `${member.name} (вы)` : `${member.name}, громкость ${localMuted ? "заглушен" : `${volume}%`}`}
      >
        <Avatar member={member} size={24} speaking={speaking && !micMuted && !deafened && !member.silenced} />
        <span className="member__name" data-speaking={speaking || undefined}>
          {member.name}
        </span>
        {member.role === "admin" && <Icon name="crown" size={14} label="Админ сервера" className="member__admin" />}
        {member.role === "moderator" && (
          <Icon name="shield" size={14} label="Модератор сервера" className="member__moderator" />
        )}
        {!isSelf && (localMuted || volume !== DEFAULT_VOLUME) && (
          <span className="member__volume">{localMuted ? <Icon name="speakerOff" size={14} label="Заглушен для вас" /> : `${volume}%`}</span>
        )}
        {member.streaming &&
          (isSelf ? (
            <span className="live-badge">LIVE</span>
          ) : (
            // Not a <button>: the row itself is one, and buttons do not nest.
            <span
              className="live-badge live-badge--watch"
              role="button"
              tabIndex={0}
              aria-label={`Смотреть стрим: ${member.name}`}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                void voice.watchStream(userId, channelId);
              }}
              onKeyDown={(e) => {
                if (e.key !== "Enter" && e.key !== " ") return;
                e.preventDefault();
                e.stopPropagation();
                void voice.watchStream(userId, channelId);
              }}
            >
              LIVE
            </span>
          ))}
        {member.silenced ? (
          <Icon name="mic" off size={15} label="Говорить запрещено" className="member__state member__state--silenced" />
        ) : deafened ? (
          <Icon name="headphones" off size={15} label="Звук выключен" className="member__state" />
        ) : (
          micMuted && <Icon name="mic" off size={15} label="Микрофон выключен" className="member__state" />
        )}
      </button>
    </li>
  );
}

/** A member's picture (ADR-0035), or their initial on their colour until it arrives. */
