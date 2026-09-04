import { JSDOM } from "jsdom";

const dom = new JSDOM("<!doctype html><html><head></head><body></body></html>", {
  url: "http://localhost/",
});
const window = dom.window;

Object.assign(globalThis, {
  window,
  document: window.document,
  navigator: window.navigator,
  HTMLElement: window.HTMLElement,
  MutationObserver: window.MutationObserver,
  getComputedStyle: window.getComputedStyle,
  IS_REACT_ACT_ENVIRONMENT: true,
});

for (const property of Object.getOwnPropertyNames(window)) {
  if (property in globalThis) continue;
  Object.defineProperty(globalThis, property, {
    configurable: true,
    enumerable: true,
    get: () => Reflect.get(window, property),
  });
}
