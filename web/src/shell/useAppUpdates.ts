import { useCallback, useEffect, useRef, useState } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { toast } from "@/state/toast";

export type AppUpdateStatus =
  | { kind: "unavailable" }
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up-to-date" }
  | { kind: "available"; version: string; notes?: string }
  | { kind: "downloading"; version: string; progress?: number }
  | { kind: "installing"; version: string }
  | { kind: "failed"; reason: "check" | "install" };

function isDesktopApp(): boolean {
  return "__TAURI__" in window;
}

export function useAppUpdates() {
  const [updateStatus, setUpdateStatus] = useState<AppUpdateStatus>({ kind: "idle" });
  const updateRef = useRef<Update | undefined>(undefined);

  const checkForUpdates = useCallback(async (announce = true) => {
    if (!isDesktopApp()) {
      setUpdateStatus({ kind: "unavailable" });
      return;
    }

    setUpdateStatus({ kind: "checking" });
    try {
      const update = await check();
      const previous = updateRef.current;
      updateRef.current = update ?? undefined;
      if (previous && previous !== update) void previous.close();

      if (!update) {
        setUpdateStatus({ kind: "up-to-date" });
        if (announce) toast.success("Oga is up to date");
        return;
      }

      setUpdateStatus({ kind: "available", version: update.version, notes: update.body });
    } catch {
      setUpdateStatus({ kind: "failed", reason: "check" });
      if (announce) toast.error("Couldn't check for updates", { description: "Try again in a moment." });
    }
  }, []);

  const installUpdate = useCallback(async () => {
    const update = updateRef.current;
    if (!update) return;

    let downloaded = 0;
    let contentLength: number | undefined;
    setUpdateStatus({ kind: "downloading", version: update.version });
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          contentLength = event.data.contentLength;
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          const progress = contentLength ? Math.min(100, Math.round((downloaded / contentLength) * 100)) : undefined;
          setUpdateStatus({ kind: "downloading", version: update.version, progress });
        } else {
          setUpdateStatus({ kind: "installing", version: update.version });
        }
      });
      await relaunch();
    } catch {
      setUpdateStatus({ kind: "failed", reason: "install" });
      toast.error("Couldn't install the update", { description: "Try again in a moment." });
    }
  }, []);

  useEffect(() => {
    void checkForUpdates(false);
    return () => {
      void updateRef.current?.close();
    };
  }, [checkForUpdates]);

  return { updateStatus, checkForUpdates, installUpdate };
}
