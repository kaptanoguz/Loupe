<script lang="ts">
  import type { Histogram } from "../api/types";

  interface Props {
    histogram: Histogram | null;
  }

  let { histogram }: Props = $props();

  const W = 256;
  const H = 72;

  /** Kanal → kapalı alan yolu. Ölçek: tüm kanalların en büyüğü (tepe kırpılır). */
  function area(values: number[], max: number): string {
    if (values.length === 0 || max <= 0) return "";
    const step = W / values.length;
    let d = `M0 ${H}`;
    values.forEach((v, i) => {
      const y = H - Math.min(1, v / max) * H;
      d += ` L${(i * step).toFixed(2)} ${y.toFixed(2)} L${((i + 1) * step).toFixed(2)} ${y.toFixed(2)}`;
    });
    return `${d} L${W} ${H} Z`;
  }

  const paths = $derived.by(() => {
    if (!histogram) return null;
    const all = [...histogram.red, ...histogram.green, ...histogram.blue, ...histogram.luma];
    // Tek bir uç değer (ör. saf siyah gökyüzü) eğriyi ezmesin: 98. yüzdelik.
    const sorted = all.sort((a, b) => a - b);
    const max = Math.max(1, sorted[Math.floor(sorted.length * 0.98)] ?? 1);
    return {
      luma: area(histogram.luma, max),
      red: area(histogram.red, max),
      green: area(histogram.green, max),
      blue: area(histogram.blue, max),
    };
  });
</script>

<figure class="relative h-18 w-full border-b border-line bg-canvas" aria-label="Histogram">
  {#if paths && histogram}
    <svg viewBox="0 0 {W} {H}" preserveAspectRatio="none" class="h-full w-full">
      <path d={paths.luma} class="fill-ink/15" />
      <path d={paths.red} class="fill-none stroke-label-red" stroke-width="1" vector-effect="non-scaling-stroke" />
      <path d={paths.green} class="fill-none stroke-label-green" stroke-width="1" vector-effect="non-scaling-stroke" />
      <path d={paths.blue} class="fill-none stroke-label-blue" stroke-width="1" vector-effect="non-scaling-stroke" />
    </svg>
    {#if histogram.shadowsClippedPermille > 0}
      <span
        class="absolute top-1 left-1 size-1.5 bg-amber"
        title="Gölgelerde kırpılma: ‰{histogram.shadowsClippedPermille}"
      ></span>
    {/if}
    {#if histogram.highlightsClippedPermille > 0}
      <span
        class="absolute top-1 right-1 size-1.5 bg-amber"
        title="Parlak alanlarda kırpılma: ‰{histogram.highlightsClippedPermille}"
      ></span>
    {/if}
  {:else}
    <span class="data absolute inset-0 flex items-center justify-center text-2xs text-ink-3">histogram yok</span>
  {/if}
</figure>
