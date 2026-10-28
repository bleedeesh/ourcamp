import { useEffect, useRef } from "react";
import { BottomPanel } from "./BottomPanel.tsx";
import { CampHeader } from "./CampHeader.tsx";
import { ChannelTree } from "./ChannelTree.tsx";
import { EmptyState } from "./EmptyState.tsx";
import { JoinPanel } from "./JoinPanel.tsx";
import { TitleBar } from "./TitleBar.tsx";
import { VolumeMenu } from "./VolumeMenu.tsx";
import { useCamps, useCampsController } from "./camps-context.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";
import { Icon } from "./icons.tsx";

export function App() {
  const view = useCamps((s) => s.view);
  return (
    <div className="app">
      <TitleBar />
      <main className="app__body">
        {view === "empty" && <EmptyState />}
        {view === "join" && <JoinPanel />}
        {view === "camp" && (
          <>
            <CampHeader />
            <ChannelTree />
            <BottomPanel />
          </>
        )}
      </main>
      <VolumeMenu />
      <Toasts />
    </div>
  );
}

/** One message at a time: errors first, then confirmations. */
function Toasts() {
  const voice = useVoiceController();
  const camps = useCampsController();
  const voiceError = useVoice((s) => s.error);
  const campError = useCamps((s) => s.error);
  const notice = useCamps((s) => s.notice);

  if (voiceError) return <Toast text={voiceError} onClose={() => voice.dismissError()} />;
  if (campError) return <Toast text={campError} onClose={() => camps.dismissError()} />;
  if (notice) return <Toast text={notice} info onClose={() => camps.dismissNotice()} />;
  return null;
}

/** Confirmations fade on their own; errors stay until closed. */
const NOTICE_MS = 4000;

function Toast({ text, info = false, onClose }: { text: string; info?: boolean; onClose: () => void }) {
  // The latest `onClose`, so a re-render does not restart the timer.
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    if (!info) return;
    const timer = setTimeout(() => close.current(), NOTICE_MS);
    return () => clearTimeout(timer);
  }, [info, text]);
  return (
    <div className={info ? "toast toast--info" : "toast"} role={info ? "status" : "alert"}>
      <span>{text}</span>
      <button type="button" className="toast__close" aria-label="Закрыть" onClick={onClose}>
        <Icon name="close" size={14} />
      </button>
    </div>
  );
}
