// A plain value in a box that can be watched. No reducer framework: every
// controller in this module holds one of these and replaces its snapshot
// wholesale on each update, the same shape the Leptos signal had.

type Listener = () => void;

export class Store<T> {
  private state: T;
  private readonly listeners = new Set<Listener>();

  constructor(initial: T) {
    this.state = initial;
  }

  get snapshot(): T {
    return this.state;
  }

  set(next: T): void {
    this.state = next;
    for (const listener of Array.from(this.listeners)) listener();
  }

  update(updater: (state: T) => T): void {
    this.set(updater(this.state));
  }

  /** Matches `useSyncExternalStore`'s subscribe signature. */
  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
}
