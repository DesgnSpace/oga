import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><head></head><body></body></html>", {
  url: "http://localhost/",
});
const window = dom.window;

/** jsdom has no resize observation; the code viewer measures its own columns. */
class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

// jsdom runs no frames: a frame is the next task on the queue instead, and a
// cancelled one never runs, so a component waiting on a row that is still
// mounting waits as long as it would in a browser.
const frames = new Map<number, ReturnType<typeof setTimeout>>();
let nextFrame = 0;

function requestFrame(callback: (time: number) => void): number {
  nextFrame += 1;
  const handle = nextFrame;
  frames.set(handle, setTimeout(() => {
    frames.delete(handle);
    callback(Date.now());
  }, 0));
  return handle;
}

function cancelFrame(handle: number): void {
  const timer = frames.get(handle);
  if (timer === undefined) return;
  clearTimeout(timer);
  frames.delete(handle);
}

Object.assign(globalThis, {
  ResizeObserver: ResizeObserverStub,
  window,
  document: window.document,
  navigator: window.navigator,
  HTMLElement: window.HTMLElement,
  MutationObserver: window.MutationObserver,
  getComputedStyle: window.getComputedStyle,
  IS_REACT_ACT_ENVIRONMENT: true,
  requestAnimationFrame: requestFrame,
  cancelAnimationFrame: cancelFrame,
});

for (const property of Object.getOwnPropertyNames(window)) {
  if (property in globalThis) continue;
  Object.defineProperty(globalThis, property, {
    configurable: true,
    enumerable: true,
    get: () => Reflect.get(window, property),
  });
}

// Nothing is laid out, so nothing can be scrolled to.
Element.prototype.scrollIntoView ??= () => {};
