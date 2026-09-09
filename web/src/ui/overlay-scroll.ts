// Marks scrollable panes while they move so the overlay scrollbar can reveal
// itself on scroll and fade out shortly after the pane stops.
const FADE_DELAY_MS = 900;

const pending = new WeakMap<Element, ReturnType<typeof setTimeout>>();

export function markOverlayScrolling(target: EventTarget | null): void {
  if (!(target instanceof Element)) return;
  target.classList.add("is-scrolling");
  const timer = pending.get(target);
  if (timer !== undefined) clearTimeout(timer);
  pending.set(
    target,
    setTimeout(() => {
      pending.delete(target);
      target.classList.remove("is-scrolling");
    }, FADE_DELAY_MS),
  );
}

export function installOverlayScroll(root: Document | Element = document): void {
  root.addEventListener("scroll", (event) => markOverlayScrolling(event.target), {
    capture: true,
    passive: true,
  });
}
