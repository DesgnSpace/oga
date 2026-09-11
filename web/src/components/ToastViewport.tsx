import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore, type CSSProperties } from "react";
import { toast, type ToastRecord } from "@/state/toast";
import {
  CloseIcon,
  ToastErrorIcon,
  ToastInfoIcon,
  ToastPendingIcon,
  ToastSuccessIcon,
} from "@/ui/icons";

const EMPTY_TOASTS: ToastRecord[] = [];
const DISMISS_DISTANCE = 100;
const DISMISS_VELOCITY = 0.35;

function toastRole(kind: ToastRecord["kind"]): "alert" | "status" {
  return kind === "error" ? "alert" : "status";
}

function ToastIcon({ kind }: { kind: ToastRecord["kind"] }) {
  if (kind === "pending") {
    return <span className="toast-icon toast-icon-pending" aria-hidden="true"><ToastPendingIcon /></span>;
  }
  if (kind === "success") {
    return <span className="toast-icon toast-icon-success" aria-hidden="true"><ToastSuccessIcon /></span>;
  }
  if (kind === "error") {
    return <span className="toast-icon toast-icon-error" aria-hidden="true"><ToastErrorIcon /></span>;
  }
  return <span className="toast-icon toast-icon-info" aria-hidden="true"><ToastInfoIcon /></span>;
}

function ToastCard({ record, onClose }: { record: ToastRecord; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const [entered, setEntered] = useState(false);
  const [dragX, setDragX] = useState(0);
  const drag = useRef<{ pointerId: number; startX: number } | null>(null);
  useEffect(() => setEntered(true), []);
  return (
    <article
      ref={ref}
      className={`toast-card toast-card-${record.kind} toast-card-${record.phase}${entered ? "" : " toast-card-entering"}`}
      data-toast-id={record.id}
      // SAFETY: React accepts CSS custom properties through the style object.
      style={{ "--toast-x": `${dragX}px` } as CSSProperties}
      onPointerDown={(event) => {
        if (event.target instanceof HTMLElement && event.target.closest("button, a, summary")) return;
        drag.current = { pointerId: event.pointerId, startX: event.clientX };
        ref.current?.setPointerCapture?.(event.pointerId);
      }}
      onPointerMove={(event) => {
        if (drag.current?.pointerId === event.pointerId) setDragX(event.clientX - drag.current.startX);
      }}
      onPointerUp={(event) => {
        if (drag.current?.pointerId !== event.pointerId) return;
        const distance = event.clientX - drag.current.startX;
        drag.current = null;
        if (Math.abs(distance) > DISMISS_DISTANCE || Math.abs(distance) > (ref.current?.offsetWidth ?? 0) * DISMISS_VELOCITY) onClose();
        else setDragX(0);
      }}
      onPointerCancel={() => { drag.current = null; setDragX(0); }}
      role={toastRole(record.kind)}
      aria-live={record.kind === "error" ? "assertive" : "polite"}
      onTransitionEnd={(event) => {
        if (event.target === ref.current && record.phase === "exiting") onClose();
      }}
    >
      <ToastIcon kind={record.kind} />
      <div className="toast-content">
        <strong>{record.title}</strong>
        {record.description ? <p>{record.description}</p> : null}
        {record.detail ? (
          <details className="toast-detail">
            <summary>Show details</summary>
            <p>{record.detail}</p>
          </details>
        ) : null}
        {record.action ? <button className="toast-action" type="button" onClick={record.action.onClick}>{record.action.label}</button> : null}
      </div>
      <button className="toast-close" type="button" aria-label="Dismiss notification" onClick={onClose}>
        <CloseIcon size={14} />
      </button>
    </article>
  );
}

export function ToastViewport() {
  const records = useSyncExternalStore(toast.subscribe.bind(toast), () => toast.snapshot, () => EMPTY_TOASTS);
  const [expanded, setExpanded] = useState(false);
  const [paused, setPaused] = useState(false);
  const interaction = useRef({ hovered: false, focused: false });
  const [rendered, setRendered] = useState<ToastRecord[]>([]);
  const [positions, setPositions] = useState<Record<number, number>>({});
  const [stackHeight, setStackHeight] = useState(0);
  const itemRefs = useRef(new Map<number, HTMLDivElement>());
  const more = Math.max(0, rendered.length - 3);
  const errorCount = rendered.filter((record) => record.kind === "error").length;

  useEffect(() => {
    setRendered((current) => {
      const exiting = current.filter((record) => record.phase === "exiting" && !records.some((next) => next.id === record.id));
      return [...records, ...exiting];
    });
  }, [records]);

  useLayoutEffect(() => {
    const next: Record<number, number> = {};
    let offset = 0;
    for (let index = rendered.length - 1; index >= 0; index -= 1) {
      const record = rendered[index];
      next[record.id] = offset;
      offset += itemRefs.current.get(record.id)?.offsetHeight ?? 0;
      if (index > 0) offset += 8;
    }
    setPositions(next);
    const front = rendered.length ? itemRefs.current.get(rendered[rendered.length - 1].id)?.offsetHeight ?? 0 : 0;
    // Collapsed peek is 12px per toast (like Sonner/pluk's --space-md), expanded gap is 8px (--space-sm).
    const collapsedPeek = 12;
    const overflowRoom = more ? 16 : 0;
    setStackHeight(expanded ? offset : front + Math.min(rendered.length, 3) * collapsedPeek + overflowRoom);
  }, [expanded, rendered, more]);

  useEffect(() => toast.onDismiss((record) => {
    setRendered((current) => [
      ...current.filter((item) => item.id !== record.id),
      { ...record, phase: "exiting" },
    ]);
  }), []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && document.activeElement instanceof HTMLElement && document.activeElement.closest(".toast-card")) {
        const card = document.activeElement.closest<HTMLElement>(".toast-card");
        const id = Number(card?.dataset.toastId);
        if (Number.isFinite(id)) toast.dismiss(id);
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  const setStackInteraction = (kind: "hovered" | "focused", value: boolean) => {
    interaction.current[kind] = value;
    const nextPaused = interaction.current.hovered || interaction.current.focused;
    setPaused(nextPaused);
    if (nextPaused) toast.pause();
    else toast.resume();
  };

  return (
    <section
      className={`toast-viewport${expanded ? " toast-viewport-expanded" : ""}`}
      // SAFETY: CSSProperties contains the numeric height used by the viewport.
      style={{ height: stackHeight } as CSSProperties}
      role="region"
      aria-label="Notifications"
      onMouseEnter={() => { setExpanded(true); setStackInteraction("hovered", true); }}
      onMouseLeave={() => { setStackInteraction("hovered", false); if (!interaction.current.focused) setExpanded(false); }}
      onFocusCapture={() => { setExpanded(true); setStackInteraction("focused", true); }}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setStackInteraction("focused", false);
          if (!interaction.current.hovered) setExpanded(false);
        }
      }}
    >
      {rendered.map((record, index) => {
        const depth = rendered.length - 1 - index;
        const collapsedScale = 1 - Math.min(depth, 3) * 0.04;
        return (
        <div
          key={record.id}
          ref={(element) => {
            if (element) itemRefs.current.set(record.id, element);
            else itemRefs.current.delete(record.id);
          }}
          className={`toast-item${!expanded && index < rendered.length - 3 ? " toast-item-hidden" : ""}`}
          // SAFETY: React accepts CSS custom properties through the style object.
          style={{ "--toast-y": `${expanded ? positions[record.id] ?? 0 : depth * 12}px`, "--toast-scale": String(expanded ? 1 : collapsedScale), zIndex: index + 1 } as CSSProperties}
        >
          <ToastCard record={record} onClose={() => {
            toast.dismiss(record.id);
            if (record.phase === "exiting") {
              toast.completeDismiss(record.id);
              setRendered((current) => current.filter((item) => item.id !== record.id));
            }
          }} />
        </div>
        );
      })}
      {!expanded && more > 0 ? <button className="toast-more" type="button" onClick={() => setExpanded(true)}>+{more} more</button> : null}
      {expanded && errorCount > 1 ? (
        <button className="toast-clear-all" type="button" onClick={() => toast.clear()}>Clear all</button>
      ) : null}
      {paused ? <span className="visually-hidden">Notifications paused</span> : null}
    </section>
  );
}
