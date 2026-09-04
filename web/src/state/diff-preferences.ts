import * as React from "react";
import { readStorage, writeStorage } from "./storage";

export type DiffView = "unified" | "split";

const DIFF_VIEW_KEY = "diffView";
const listeners = new Set<() => void>();

export function loadDiffView(): DiffView {
  return readStorage(DIFF_VIEW_KEY) === "split" ? "split" : "unified";
}

export function storeDiffView(view: DiffView): void {
  writeStorage(DIFF_VIEW_KEY, view);
  listeners.forEach((listener) => listener());
}

function subscribeDiffView(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useDiffView(): [DiffView, (view: DiffView) => void] {
  const view = React.useSyncExternalStore(subscribeDiffView, loadDiffView, (): DiffView => "unified");
  return [view, storeDiffView];
}
