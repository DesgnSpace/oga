import { Component, StrictMode, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import App from "@/App";
import { installNativeChrome } from "@/shell/nativeChrome";
import "@/oga.css";
import "./index.css";

class StartupErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("Desktop app failed to render", error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <main className="app-shell" role="alert">
          <h1>Oga could not load</h1>
          <p>{this.state.error.message}</p>
          <button type="button" onClick={() => window.location.reload()}>Reload Oga</button>
        </main>
      );
    }
    return this.props.children;
  }
}

function applyPlatformClass(): void {
  const platform = navigator.userAgent.toLowerCase();
  if (platform.includes("mac")) {
    document.documentElement.classList.add("platform-macos");
  }
}

applyPlatformClass();
installNativeChrome();

const root = document.getElementById("root");
if (!root) throw new Error("Missing #root element");

createRoot(root).render(
  <StartupErrorBoundary>
    <StrictMode>
      <App />
    </StrictMode>
  </StartupErrorBoundary>,
);
