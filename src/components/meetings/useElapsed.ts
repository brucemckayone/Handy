import { useEffect, useState } from "react";

/** Milliseconds since `startedAt` (unix ms), refreshed every second. */
export const useElapsed = (startedAt: number | null | undefined): number => {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (startedAt == null) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [startedAt]);
  return startedAt == null ? 0 : Math.max(0, now - startedAt);
};
