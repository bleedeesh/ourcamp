import type { MouseEvent } from "react";
import type { Channel, Member, UserId } from "../domain/types.ts";
import { DEFAULT_VOLUME } from "../domain/volume.ts";
import { Campfire } from "@ourcamp/ui/Campfire.tsx";
import { Icon } from "./icons.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";

export function ChannelTree() {
  const channels = useVoice((s) => s.camp.channels);
  return (
    <nav className="tree" aria-label="Голосовые каналы">
      {channels.map((channel) => (
        <ChannelNode key={channel.id} channel={channel} />
      ))}
    </nav>
  );
}

function ChannelNode({ channel }: { channel: Channel }) {
  const voice = useVoiceController();
  const current = useVoice((s) => s.channelId === channel.id);
  const selfId = useVoice((s) => s.selfId);
  const memberIds = current ? [...channel.memberIds, selfId] : channel.memberIds;
  const lit = memberIds.length > 0;

  return (
    <div className="channel">
      <button
        type="button"
        className="channel__row"
        aria-current={current ? "true" : undefined}
        onClick={() => void voice.join(channel.id)}
      >
        <Campfire size={20} simple={!lit} dim={!lit} />
        <span className="channel__name">{channel.name}</span>
        {lit && <span className="channel__count">{memberIds.length}</span>}
      </button>
      {lit && (
        <ul className="channel__members">
          {memberIds.map((id) => (
            <MemberRow key={id} userId={id} />
          ))}
        </ul>
      )}
    </div>
  );
}

function MemberRow({ userId }: { userId: UserId }) {
  const voice = useVoiceController();
  const member = useVoice((s) => s.camp.members[userId]);
  const isSelf = useVoice((s) => s.selfId === userId);
  const speaking = useVoice((s) => s.speaking[userId] === true);
  const selfMicMuted = useVoice((s) => s.micMuted);
  const selfDeafened = useVoice((s) => s.deafened);
  const prefs = useVoice((s) => s.prefs[userId]);

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
        onClick={openMenu}
        onContextMenu={openMenu}
        aria-label={isSelf ? `${member.name} (вы)` : `${member.name}, громкость ${localMuted ? "заглушен" : `${volume}%`}`}
      >
        <Avatar member={member} size={24} speaking={speaking && !micMuted && !deafened} />
        <span className="member__name" data-speaking={speaking || undefined}>
          {member.name}
        </span>
        {member.isAdmin && <Icon name="crown" size={14} label="Админ лагеря" className="member__admin" />}
        {!isSelf && (localMuted || volume !== DEFAULT_VOLUME) && (
          <span className="member__volume">{localMuted ? <Icon name="speakerOff" size={14} label="Заглушен для вас" /> : `${volume}%`}</span>
        )}
        {member.streaming && <span className="live-badge">LIVE</span>}
        {deafened ? (
          <Icon name="headphones" off size={15} label="Звук выключен" className="member__state" />
        ) : (
          micMuted && <Icon name="mic" off size={15} label="Микрофон выключен" className="member__state" />
        )}
      </button>
    </li>
  );
}

export function Avatar({ member, size, speaking }: { member: Member; size: number; speaking: boolean }) {
  return (
    <span
      className="avatar"
      data-speaking={speaking || undefined}
      style={{ width: size, height: size, background: member.color, fontSize: Math.round(size * 0.46) }}
    >
      {member.name.slice(0, 1)}
    </span>
  );
}
