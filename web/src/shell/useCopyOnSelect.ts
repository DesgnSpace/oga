import { useEffect } from "react";
import { copyText } from "@/lib/identifiers";
import { toast } from "@/state/toast";
import { isTextField } from "./useKeyboardShortcuts";

/** Text fields are skipped so selecting a word to replace it keeps the clipboard. */
export function useCopyOnSelect(): void {
  useEffect(() => {
    let selectionAtPress = "";
    let pressedInField = false;

    const handleMouseDown = (event: MouseEvent) => {
      selectionAtPress = window.getSelection()?.toString() ?? "";
      pressedInField = isTextField(event.target);
    };
    const handleMouseUp = (event: MouseEvent) => {
      if (event.button !== 0 || pressedInField || isTextField(document.activeElement)) return;
      const selected = window.getSelection()?.toString() ?? "";
      if (selected.trim() === "" || selected === selectionAtPress) return;
      copyText(selected).then(
        () => toast.success("Copied to clipboard"),
        () => toast.error("Couldn't copy the selection"),
      );
    };

    document.addEventListener("mousedown", handleMouseDown);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousedown", handleMouseDown);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, []);
}
