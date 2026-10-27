import { BottomPanel } from "./BottomPanel.tsx";
import { CampHeader } from "./CampHeader.tsx";
import { ChannelTree } from "./ChannelTree.tsx";
import { TitleBar } from "./TitleBar.tsx";
import { VolumeMenu } from "./VolumeMenu.tsx";
import { useVoice, useVoiceController } from "./voice-context.tsx";
import { Icon } from "./icons.tsx";

export function App() {
  return (
    <div className="app">
      <TitleBar />
      <main className="app__body">
        <CampHeader />
        <ChannelTree />
        <BottomPanel />
      </main>
      <VolumeMenu />
      <ErrorToast />
    </div>
  );
}

function ErrorToast() {
  const voice = useVoiceController();
  const error = useVoice((s) => s.error);
  if (!error) return null;
  return (
    <div className="toast" role="alert">
      <span>{error}</span>
      <button type="button" className="toast__close" aria-label="Закрыть" onClick={() => voice.dismissError()}>
        <Icon name="close" size={14} />
      </button>
    </div>
  );
}
