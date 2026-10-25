<script lang="ts">
  import type { FaceRef, ThumbSize } from "../api/types";
  import { faceCrop } from "../design/face";
  import type { Catalog } from "../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
    face: FaceRef | null;
    width: number;
    height: number;
    size?: ThumbSize;
  }

  let { catalog, face, width, height, size = "lg" }: Props = $props();

  const crop = $derived(face ? faceCrop(face, width, height) : null);
</script>

<!-- Kare ya da portre, 2px köşe: kişi kapakları asla daire değildir. -->
<div class="relative shrink-0 overflow-hidden rounded-xs bg-raised" style:width="{width}px" style:height="{height}px">
  {#if face && crop}
    <img
      src={catalog.backend.thumbUrl(size, face.hash)}
      alt=""
      loading="lazy"
      draggable="false"
      class="absolute max-w-none"
      style:width="{crop.width}px"
      style:height="{crop.height}px"
      style:left="{crop.left}px"
      style:top="{crop.top}px"
    />
  {/if}
</div>
