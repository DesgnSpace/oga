import { hasDesktopBridge } from "@/bridge";
import { AppShell } from "@/shell/AppShell";

export default function App() {
  return (
    <>
      {!hasDesktopBridge() && (
        <div className="dev-mode-banner" role="status">
          No desktop connection — task data won't load here.
        </div>
      )}
      <AppShell />
    </>
  );
}
