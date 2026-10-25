/// <reference lib="webworker" />
/**
 * Düzen hesabı ana iş parçacığının dışında: 100k öğede bile kaydırma ve
 * klavye takılmaz. Mesaj başına bir hesap; eski istekler `seq` ile atlanır.
 */
import { type JustifiedInput, justify } from "./justified";

export interface LayoutRequest {
  seq: number;
  input: JustifiedInput;
}

self.onmessage = (ev: MessageEvent<LayoutRequest>) => {
  const { seq, input } = ev.data;
  const t0 = performance.now();
  const layout = justify(input);
  const ms = performance.now() - t0;
  (self as unknown as DedicatedWorkerGlobalScope).postMessage({ seq, layout, ms }, [
    layout.rects.buffer,
    layout.rowTops.buffer,
    layout.rowHeights.buffer,
    layout.rowFirst.buffer,
    layout.headerTops.buffer,
  ]);
};
