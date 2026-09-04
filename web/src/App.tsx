import { AppShell } from "@/shell/AppShell";

function hasDesktopBridge(): boolean {
  return typeof window !== "undefined" && "__TAURI__" in window;
}

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
