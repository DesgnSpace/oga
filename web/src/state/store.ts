// A plain observable value; each update replaces the snapshot.

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

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }
}
