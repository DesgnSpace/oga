// The native shell reports every hide and show; the page stops drawing while
// hidden and catches up on show.

import { useEffect, useRef, useSyncExternalStore } from "react";
import { mainWindowVisible } from "@/bridge/client";
import { onWindowVisibility } from "@/bridge/events";
import { hasDesktopBridge } from "@/bridge/transport";

type Listener = () => void;

let visible = true;
let started = false;
let reported = false;
let unsubscribeBridge: (() => void) | undefined;
const listeners = new Set<Listener>();
const shownListeners = new Set<Listener>();

function announceShown(): void {
  for (const listener of Array.from(shownListeners)) listener();
}

function announce(): void {
  for (const listener of Array.from(listeners)) listener();
}

function report(next: boolean): void {
  reported = true;
  if (next === visible) {
    if (next) announceShown();
    return;
  }
  visible = next;
  announce();
  if (next) announceShown();
}

function attach(): void {
  if (started) return;
  started = true;
  unsubscribeBridge = onWindowVisibility((next) => report(next.visible));
  if (!hasDesktopBridge()) return;
  void mainWindowVisible().then((result) => {
    if (reported || !result.ok || result.value === visible) return;
    visible = result.value;
    announce();
  });
}

export function isWindowVisible(): boolean {
  return visible;
}

export function subscribeWindowVisibility(listener: Listener): () => void {
  listeners.add(listener);
  attach();
  return () => {
    listeners.delete(listener);
  };
}

/** Runs on every show, including one where the page never saw the hide. */
export function subscribeWindowShown(listener: Listener): () => void {
  shownListeners.add(listener);
  attach();
  return () => {
    shownListeners.delete(listener);
  };
}

export function useWindowVisible(): boolean {
  return useSyncExternalStore(subscribeWindowVisibility, isWindowVisible, () => true);
}

export function useWindowShown(handler: () => void): void {
  const current = useRef(handler);
  current.current = handler;
  useEffect(() => subscribeWindowShown(() => current.current()), []);
}

export function resetWindowVisibilityForTests(): void {
  unsubscribeBridge?.();
  unsubscribeBridge = undefined;
  started = false;
  reported = false;
  visible = true;
  listeners.clear();
  shownListeners.clear();
}
