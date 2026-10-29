import { useAvatarUrl } from "./avatar-url.ts";

/** Who an avatar shows: a camp member in the tree, or an author in the chat. */
export interface AvatarPerson {
  readonly id: string;
  readonly name: string;
  readonly color: string;
  /** Avatar hash (ADR-0035); `null`: the initial on their colour. */
  readonly avatar: string | null;
}

/** A round avatar: the member's picture, or their initial on their colour. */
export function Avatar({ member, size, speaking }: { member: AvatarPerson; size: number; speaking: boolean }) {
  const picture = useAvatarUrl(member.id, member.avatar);
  return (
    <span
      className="avatar"
      data-speaking={speaking || undefined}
      style={{ width: size, height: size, background: member.color, fontSize: Math.round(size * 0.46) }}
    >
      {picture === null ? member.name.slice(0, 1) : <img className="avatar__picture" src={picture} alt="" draggable={false} />}
    </span>
  );
}
