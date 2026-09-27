import * as React from "react";
import { copyText } from "@/lib/identifiers";
import { toast } from "@/state/toast";
import { CheckIcon, CopyIcon } from "@/ui/icons";

const COPIED_FLASH_MS = 1_500;

export function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = React.useState(false);
  const flashTimer = React.useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  React.useEffect(() => () => clearTimeout(flashTimer.current), []);

  const copy = async () => {
    try {
      await copyText(text);
      setCopied(true);
      clearTimeout(flashTimer.current);
      flashTimer.current = setTimeout(() => setCopied(false), COPIED_FLASH_MS);
    } catch {
      toast.error(`Couldn't copy the ${label}`);
    }
  };

  return (
    <button
      className="copy-button"
      type="button"
      data-copied={copied || undefined}
      aria-label={copied ? "Copied" : `Copy ${label}`}
      title={copied ? "Copied" : `Copy ${label}`}
      onClick={(event) => {
        event.stopPropagation();
        void copy();
      }}
    >
      {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
    </button>
  );
}
