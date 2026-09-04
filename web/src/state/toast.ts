import { Store } from "./store";

export type ToastKind = "success" | "error" | "info" | "pending";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastOptions {
  description?: string;
  detail?: string;
  action?: ToastAction;
  /** Retried when a pending toast stalls past {@link PENDING_STALL_MS}. */
  retry?: () => void;
}

export interface ToastRecord extends ToastOptions {
  id: number;
  kind: ToastKind;
  title: string;
  phase: "visible" | "exiting";
  remaining?: number;
  deadline?: number;
}

export interface ToastHandle {
  success(title: string, options?: ToastOptions): void;
  error(title: string, options?: ToastOptions): void;
  /** Quietly resolves a pending toast whose success is already visible elsewhere in the UI. */
  dismiss(): void;
}

type DismissListener = (toast: ToastRecord) => void;

interface ToastClock {
  now: () => number;
  setTimeout: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>;
  clearTimeout: (timer: ReturnType<typeof setTimeout>) => void;
}

const AUTO_DISMISS_MS = 4_000;
const MAX_TOASTS = 5;
const PENDING_STALL_MS = 30_000;

const browserClock: ToastClock = {
  now: () => Date.now(),
  setTimeout: (callback, delay) => setTimeout(callback, delay),
  clearTimeout: (timer) => clearTimeout(timer),
};

export class ToastStore {
  private readonly store = new Store<ToastRecord[]>([]);
  private readonly timers = new Map<number, ReturnType<typeof setTimeout>>();
  private readonly stallTimers = new Map<number, ReturnType<typeof setTimeout>>();
  private readonly dismissListeners = new Set<DismissListener>();
  private nextId = 1;

  constructor(private readonly clock: ToastClock = browserClock) {}

  get snapshot(): ToastRecord[] {
    return this.store.snapshot;
  }

  subscribe(listener: () => void): () => void {
    return this.store.subscribe(listener);
  }

  onDismiss(listener: DismissListener): () => void {
    this.dismissListeners.add(listener);
    return () => this.dismissListeners.delete(listener);
  }

  success(title: string, options?: ToastOptions): void {
    this.add("success", title, options);
  }

  error(title: string, options?: ToastOptions): void {
    this.add("error", title, options);
  }

  info(title: string, options?: ToastOptions): void {
    this.add("info", title, options);
  }

  pending(title: string, options?: ToastOptions): ToastHandle {
    const id = this.add("pending", title, options);
    const retry = options?.retry;
    this.stallTimers.set(id, this.clock.setTimeout(() => {
      this.stallTimers.delete(id);
      const current = this.snapshot.find((toast) => toast.id === id);
      if (!current || current.kind !== "pending") return;
      this.resolve(id, "error", current.title, {
        description: "This is taking longer than expected.",
        action: retry ? { label: "Retry", onClick: retry } : undefined,
      });
    }, PENDING_STALL_MS));
    return {
      success: (nextTitle, nextOptions) => { this.cancelStall(id); this.resolve(id, "success", nextTitle, nextOptions); },
      error: (nextTitle, nextOptions) => { this.cancelStall(id); this.resolve(id, "error", nextTitle, nextOptions); },
      dismiss: () => { this.cancelStall(id); this.completeDismiss(id); },
    };
  }

  clear(): void {
    for (const id of this.snapshot.map((toast) => toast.id)) {
      this.cancelTimer(id);
      this.cancelStall(id);
    }
    this.store.set([]);
  }

  dismiss(id: number): void {
    this.beginDismiss(id);
  }

  completeDismiss(id: number): void {
    if (!this.snapshot.some((toast) => toast.id === id)) return;
    this.cancelTimer(id);
    this.cancelStall(id);
    this.store.update((toasts) => toasts.filter((toast) => toast.id !== id));
  }

  pause(): void {
    const now = this.clock.now();
    for (const toast of this.snapshot) {
      if (toast.phase !== "visible" || toast.remaining === undefined) continue;
      this.cancelTimer(toast.id);
    }
    this.store.update((toasts) => toasts.map((toast) => {
      if (toast.phase !== "visible" || toast.remaining === undefined) return toast;
      if (toast.deadline === undefined) return toast;
      return { ...toast, remaining: Math.max(0, toast.deadline - now), deadline: undefined };
    }));
  }

  resume(): void {
    const now = this.clock.now();
    this.store.update((toasts) => toasts.map((toast) => {
      if (toast.phase !== "visible" || toast.remaining === undefined) return toast;
      if (toast.deadline !== undefined) return toast;
      this.schedule(toast.id, toast.remaining);
      return { ...toast, deadline: now + toast.remaining };
    }));
  }

  private add(kind: ToastKind, title: string, options?: ToastOptions): number {
    const id = this.nextId++;
    const remaining = kind === "success" || kind === "info" ? AUTO_DISMISS_MS : undefined;
    const toast = {
      id,
      kind,
      title,
      phase: "visible" as const,
      remaining,
      deadline: remaining === undefined ? undefined : this.clock.now() + remaining,
      ...options,
    };
    const next = [...this.snapshot, toast].slice(-MAX_TOASTS);
    for (const oldToast of this.snapshot.slice(0, Math.max(0, this.snapshot.length + 1 - MAX_TOASTS))) {
      this.cancelTimer(oldToast.id);
      this.cancelStall(oldToast.id);
    }
    this.store.set(next);
    if (remaining !== undefined) this.schedule(id, remaining);
    return id;
  }

  private resolve(id: number, kind: "success" | "error", title: string, options?: ToastOptions): void {
    const current = this.snapshot.find((toast) => toast.id === id);
    if (!current || current.phase === "exiting") return;
    this.cancelTimer(id);
    const remaining = kind === "success" ? AUTO_DISMISS_MS : undefined;
    const deadline = remaining === undefined ? undefined : this.clock.now() + remaining;
    this.store.update((toasts) => toasts.map((toast) => (
      toast.id === id
        ? {
            ...toast,
            ...options,
            description: options?.description,
            detail: options?.detail,
            action: options?.action,
            kind,
            title,
            phase: "visible" as const,
            remaining,
            deadline,
          }
        : toast
    )));
    if (remaining !== undefined) this.schedule(id, remaining);
  }

  private schedule(id: number, delay: number): void {
    this.cancelTimer(id);
    this.timers.set(id, this.clock.setTimeout(() => {
      this.timers.delete(id);
      this.beginDismiss(id);
    }, delay));
  }

  private beginDismiss(id: number): void {
    const current = this.snapshot.find((toast) => toast.id === id);
    if (!current) return;
    this.cancelTimer(id);
    this.cancelStall(id);
    for (const listener of Array.from(this.dismissListeners)) listener(current);
    this.store.update((toasts) => toasts.filter((toast) => toast.id !== id));
  }

  private cancelTimer(id: number): void {
    const timer = this.timers.get(id);
    if (timer === undefined) return;
    this.clock.clearTimeout(timer);
    this.timers.delete(id);
  }

  private cancelStall(id: number): void {
    const timer = this.stallTimers.get(id);
    if (timer === undefined) return;
    this.clock.clearTimeout(timer);
    this.stallTimers.delete(id);
  }
}

export const toast = new ToastStore();
