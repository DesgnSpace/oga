// Whether Oga tells the user when a task stops. On unless they turned it off,
// and pushed to the shell, which is the side that raises the notification.

import * as React from "react";
import { setTaskNotifications } from "@/bridge/client";
import { readStorage, writeStorage } from "./storage";

const TASK_NOTIFICATIONS_KEY = "taskNotifications";
const listeners = new Set<() => void>();

export function loadTaskNotifications(): boolean {
  return readStorage(TASK_NOTIFICATIONS_KEY) !== "off";
}

export function storeTaskNotifications(enabled: boolean): void {
  writeStorage(TASK_NOTIFICATIONS_KEY, enabled ? "on" : "off");
  listeners.forEach((listener) => listener());
  void setTaskNotifications(enabled);
}

function subscribeTaskNotifications(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useTaskNotifications(): [boolean, (enabled: boolean) => void] {
  const enabled = React.useSyncExternalStore(subscribeTaskNotifications, loadTaskNotifications, () => true);
  return [enabled, storeTaskNotifications];
}
