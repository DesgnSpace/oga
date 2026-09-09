declare module "bun:test" {
  interface Matchers<T> {
    toHaveBeenCalledOnce(): void;
  }
}
