/** 每个请求单独控制的 Promise：先 settle 的一次生效，后续 resolve/reject 忽略。 */

export type Deferred<T> = {
  readonly promise: Promise<T>;
  readonly settled: boolean;
  resolve: (value: T | PromiseLike<T>) => void;
  reject: (reason?: unknown) => void;
};

export function deferred<T>(): Deferred<T> {
  let settled = false;
  let resolvePromise: (value: T | PromiseLike<T>) => void;
  let rejectPromise: (reason?: unknown) => void;
  const promise = new Promise<T>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return {
    promise,
    get settled() {
      return settled;
    },
    resolve(value) {
      if (settled) return;
      settled = true;
      resolvePromise(value);
    },
    reject(reason) {
      if (settled) return;
      settled = true;
      rejectPromise(reason);
    },
  };
}
