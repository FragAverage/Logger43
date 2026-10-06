// Production stand-in for devMock.ts (aliased in vite.config.ts for `vite build`). Never called.
export async function mockInvoke<T>(): Promise<T> {
  throw new Error("dev mock is not available in production builds");
}
export async function mockListen(): Promise<() => void> {
  throw new Error("dev mock is not available in production builds");
}
