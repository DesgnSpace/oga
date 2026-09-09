// Hand-rolled client-side routing, matching the previous Rust app's router
// (rust/crates/oga-ui/src/lib.rs, src/router.rs): three routes plus a
// fallback, history push/pop with no page reload, and link clicks
// intercepted unless the user asked the platform for something else.

import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import type { SettingsTab } from "@/screens/settings/state";

export type Route =
  | { kind: "home" }
  | { kind: "task"; id: string }
  | { kind: "settings"; tab?: SettingsTab }
  | { kind: "usage" }
  | { kind: "icons" }
  | { kind: "not-found" };

export function routeFromPath(path: string): Route {
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- standard window guard for SSR
  if (typeof window !== "undefined" && window.location.hash === "#/icons") return { kind: "icons" };
  const normalized = path.split("#")[0] ?? path;
  const withoutQuery = normalized.split("?")[0] ?? normalized;
  const trimmed = withoutQuery.replace(/\/+$/, "");
  if (trimmed === "") return { kind: "home" };
  if (trimmed === "/settings") return { kind: "settings" };
  if (trimmed === "/usage") return { kind: "usage" };
  if (trimmed === "/icons") return { kind: "icons" };
  if (trimmed.startsWith("/tasks/") && trimmed.length > "/tasks/".length) {
    return { kind: "task", id: trimmed.slice("/tasks/".length) };
  }
  return { kind: "not-found" };
}

export function routePath(route: Route): string {
  switch (route.kind) {
    case "home":
    case "not-found":
      return "/";
    case "task":
      return `/tasks/${route.id}`;
    case "settings":
      return "/settings";
    case "usage":
      return "/usage";
    case "icons":
      return "/icons";
  }
}

function currentPath(): string {
  // oxlint-disable-next-line anti-slop/no-runtime-typeof -- standard window guard for SSR
  if (typeof window === "undefined") return "/";
  if (window.location.hash === "#/icons") return "/icons";
  return window.location.pathname + window.location.search;
}

/** Whether a click should be handled in-app rather than left to the platform. */
export function handlesClick(event: React.MouseEvent): boolean {
  return event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;
}

interface RouterContextValue {
  route: Route;
  navigate: (route: Route) => void;
  canGoBack: boolean;
  canGoForward: boolean;
  goBack: () => void;
  goForward: () => void;
}

const RouterContext = createContext<RouterContextValue | undefined>(undefined);

/** Each entry this router pushes carries its depth, so back/forward
 * availability can be read from history state instead of tracked by hand. */
function historyIndex(): number {
  // SAFETY: router entries write ogaIndex as a number via pushState below.
  const state = window.history.state as { ogaIndex?: number } | null;
  if (state?.ogaIndex !== undefined) return state.ogaIndex;
  window.history.replaceState({ ogaIndex: 0 }, "", currentPath());
  return 0;
}

export function RouterProvider({ children }: { children: React.ReactNode }) {
  const [route, setRoute] = useState<Route>(() => routeFromPath(currentPath()));
  const indexRef = useRef(0);
  const maxIndexRef = useRef(0);
  const [canGoBack, setCanGoBack] = useState(false);
  const [canGoForward, setCanGoForward] = useState(false);

  useEffect(() => {
    const index = historyIndex();
    indexRef.current = index;
    maxIndexRef.current = index;
    setCanGoBack(index > 0);
  }, []);

  useEffect(() => {
    const onPopState = (event: PopStateEvent) => {
      // SAFETY: this handler only reads the optional number written by this router.
      const index = (event.state as { ogaIndex?: number } | null)?.ogaIndex ?? 0;
      indexRef.current = index;
      maxIndexRef.current = Math.max(maxIndexRef.current, index);
      setCanGoBack(index > 0);
      setCanGoForward(index < maxIndexRef.current);
      setRoute(routeFromPath(currentPath()));
    };
    const onHash = () => setRoute(routeFromPath(currentPath()));
    window.addEventListener("popstate", onPopState);
    window.addEventListener("hashchange", onHash);
    return () => {
      window.removeEventListener("popstate", onPopState);
      window.removeEventListener("hashchange", onHash);
    };
  }, []);

  const navigate = useCallback((next: Route) => {
    setRoute((current) => {
      if (routePath(current) === routePath(next) && current.kind === next.kind) return current;
      const index = indexRef.current + 1;
      indexRef.current = index;
      maxIndexRef.current = index;
      window.history.pushState({ ogaIndex: index }, "", routePath(next));
      setCanGoBack(true);
      setCanGoForward(false);
      return next;
    });
  }, []);

  const goBack = useCallback(() => window.history.back(), []);
  const goForward = useCallback(() => window.history.forward(), []);

  return (
    <RouterContext.Provider value={{ route, navigate, canGoBack, canGoForward, goBack, goForward }}>
      {children}
    </RouterContext.Provider>
  );
}

export function useRouter(): RouterContextValue {
  const context = useContext(RouterContext);
  if (!context) throw new Error("useRouter must be used within a RouterProvider");
  return context;
}
