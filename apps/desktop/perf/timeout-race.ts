/** 给 Promise.race 配超时，并在结束时清掉 timer。自包含，便于 toString 后在页内 eval。 */
export async function raceWithTimeout<T>(work: Promise<T>, ms: number, message = "timeout"): Promise<T> {
  let id: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_, reject) => {
        id = setTimeout(() => reject(new Error(message)), ms);
      }),
    ]);
  } finally {
    if (id !== undefined) clearTimeout(id);
  }
}
