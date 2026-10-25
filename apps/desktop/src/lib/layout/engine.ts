/**
 * Düzen motoru: Worker varsa orada, yoksa (jsdom testleri) aynı iş parçacığında.
 * Yalnızca en son isteğin sonucu teslim edilir.
 */
import { type JustifiedInput, type JustifiedLayout, justify } from "./justified";
import type { LayoutRequest } from "./justified.worker";

export interface LayoutResult {
  layout: JustifiedLayout;
  ms: number;
}

export class LayoutEngine {
  private worker: Worker | null = null;
  private seq = 0;
  private pending = new Map<number, (r: LayoutResult) => void>();

  constructor(useWorker = typeof Worker !== "undefined") {
    if (!useWorker) return;
    try {
      this.worker = new Worker(new URL("./justified.worker.ts", import.meta.url), { type: "module" });
      this.worker.onmessage = (ev: MessageEvent<LayoutResult & { seq: number }>) => {
        const done = this.pending.get(ev.data.seq);
        this.pending.delete(ev.data.seq);
        // Daha yeni bir istek varsa bu sonuç bayattır.
        if (done && ev.data.seq === this.seq) done({ layout: ev.data.layout, ms: ev.data.ms });
      };
    } catch {
      this.worker = null;
    }
  }

  /** Bayatlayan istekler hiç çözülmez; çağıran yalnızca en sonuncuyu bekler. */
  compute(input: JustifiedInput): Promise<LayoutResult> {
    const seq = ++this.seq;
    if (!this.worker) {
      const t0 = performance.now();
      const layout = justify(input);
      return Promise.resolve({ layout, ms: performance.now() - t0 });
    }
    for (const k of this.pending.keys()) if (k < seq) this.pending.delete(k);
    return new Promise((resolve) => {
      this.pending.set(seq, resolve);
      // aspects kopyalanır (çağıran dizisini tutmaya devam eder).
      const req: LayoutRequest = { seq, input: { ...input, aspects: input.aspects.slice() } };
      this.worker?.postMessage(req, [req.input.aspects.buffer]);
    });
  }

  dispose(): void {
    this.worker?.terminate();
    this.worker = null;
  }
}
