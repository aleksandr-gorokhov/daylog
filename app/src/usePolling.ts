import { useEffect, useRef } from "react";

/**
 * Вызывает `tick` раз в `intervalMs`, только пока окно видимо. При возвращении окна
 * (видимость или фокус) вызывает сразу, не дожидаясь интервала.
 */
export function usePolling(tick: () => void, intervalMs: number) {
  const ref = useRef(tick);
  ref.current = tick;

  useEffect(() => {
    let timer: number | undefined;
    const run = () => ref.current();
    const start = () => {
      if (timer === undefined) timer = window.setInterval(run, intervalMs);
    };
    const stop = () => {
      if (timer !== undefined) window.clearInterval(timer);
      timer = undefined;
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") {
        run();
        start();
      } else {
        stop();
      }
    };

    if (document.visibilityState === "visible") start();
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("focus", run);
    return () => {
      stop();
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("focus", run);
    };
  }, [intervalMs]);
}
