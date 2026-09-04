import * as React from "react";
import { useDiffView, type DiffView } from "@/state/diff-preferences";

export function DiffHeader() {
  const [view, setView] = useDiffView();
  return (
    <div className="diff-view-toggle" role="group">
      {(["unified", "split"] as const).map((option: DiffView) => (
        <button
          className={`diff-view-toggle-option${view === option ? " diff-view-toggle-option-active" : ""}`}
          type="button"
          aria-pressed={view === option}
          onClick={() => setView(option)}
          key={option}
        >
          {option[0].toUpperCase() + option.slice(1)}
        </button>
      ))}
    </div>
  );
}
