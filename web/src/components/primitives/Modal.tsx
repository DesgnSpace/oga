// Reusable modal overlay. Mirrors pluk's ui/src/modal.ts pattern (overlay +
// dialog, Escape/outside-click/close-button dismissal, a hand-rolled Tab
// focus trap, body scroll lock, focus moved in on open and restored on
// close) adapted to React: state-driven mount/unmount instead of imperative
// DOM creation, since callers here are components, not one-shot triggers.

import { useEffect, useRef, type ReactNode } from "react";
import { CloseIcon } from "@/ui/icons";

const FOCUSABLE_SELECTOR = [
  "button:not([disabled])",
  "[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

// Open dialogs mark the body so the page layer behind them can stand down
// (its overlay scrollbars would otherwise paint above the dim). Counted:
// dialogs stack, and the mark lifts only when the last one closes.
let openModalCount = 0;

function markModalOpen(): void {
  openModalCount += 1;
  if (openModalCount === 1) document.body.classList.add("modal-open");
}

function markModalClosed(): void {
  openModalCount = Math.max(0, openModalCount - 1);
  if (openModalCount === 0) document.body.classList.remove("modal-open");
}

export interface ModalProps {
  open: boolean;
  onClose: () => void;
  /** id of an element inside `children` that names the dialog, e.g. its heading. */
  labelledBy: string;
  children: ReactNode;
  className?: string;
}
export function Modal({ open, onClose, labelledBy, children, className }: ModalProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) return;
    openerRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    markModalOpen();

    const shouldLockBodyScroll = getComputedStyle(document.body).overflow !== "hidden";
    const previousOverflow = shouldLockBodyScroll ? document.body.style.overflow : undefined;
    if (shouldLockBodyScroll) document.body.style.overflow = "hidden";

    const focusable = () => Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR) ?? []);
    (focusable()[0] ?? dialogRef.current)?.focus();

    const onKeydown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const items = focusable();
      if (!items.length) {
        event.preventDefault();
        dialogRef.current?.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeydown);
    return () => {
      markModalClosed();
      document.removeEventListener("keydown", onKeydown);
      if (shouldLockBodyScroll) document.body.style.overflow = previousOverflow ?? "";
      openerRef.current?.focus();
    };
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div
      className="modal-overlay"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className={className ? `modal-dialog ${className}` : "modal-dialog"}
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelledBy}
        tabIndex={-1}
      >
        <button className="modal-close icon-button" type="button" aria-label="Close dialog" onClick={onClose}>
          <CloseIcon size={18} />
        </button>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}
