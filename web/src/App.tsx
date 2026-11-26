import { getTransport, hasDesktopBridge, tauriTransport } from "@/bridge";
import { AppShell } from "@/shell/AppShell";

export default function App() {
  return (
    <>
      {getTransport() === tauriTransport && !hasDesktopBridge() && (
        <div className="dev-mode-banner" role="status">
          No desktop connection — task data won't load here.
        </div>
      )}
      <AppShell />
    </>
  );
}
