/** 每条连接独立轮询，等自己的请求结束后再计时；停止后不再提交晚到的结果。 */
export function pollConnections<T>(
  connIds: number[],
  read: (connId: number) => Promise<T>,
  onValue: (connId: number, value: T) => void,
  onError: (connId: number, error: unknown) => void,
  intervalMs = 3000,
): () => void {
  let stopped = false;
  const timers = new Map<number, ReturnType<typeof setTimeout>>();
  for (const connId of new Set(connIds)) {
    const tick = async () => {
      try {
        const value = await read(connId);
        if (!stopped) onValue(connId, value);
      } catch (error) {
        if (!stopped) onError(connId, error);
      } finally {
        if (!stopped) timers.set(connId, setTimeout(() => void tick(), intervalMs));
      }
    };
    void tick();
  }
  return () => {
    stopped = true;
    for (const timer of timers.values()) clearTimeout(timer);
    timers.clear();
  };
}
