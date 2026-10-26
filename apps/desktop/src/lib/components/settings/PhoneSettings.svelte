<script lang="ts">
  import Smartphone from "@lucide/svelte/icons/smartphone";
  import Wifi from "@lucide/svelte/icons/wifi";

  import type { Catalog } from "../../state/catalog.svelte";

  interface Props {
    catalog: Catalog;
  }

  let { catalog }: Props = $props();

  const lan = $derived(catalog.status?.lan);
  const target = $derived(catalog.sidebar?.libraries.find((l) => l.primary));
  const STATE = { off: "kapalı", listening: "açık", no_library: "hedef kütüphane yok", error: "hata" } as const;
</script>

<div class="flex flex-col gap-6 p-4">
  <section class="flex flex-col gap-2">
    <h2 class="label">Wi-Fi ile aktarım (iPhone ve Android)</h2>
    <dl class="kv data text-2xs">
      <dt>Sunucu</dt>
      <dd class={lan?.state === "listening" ? "" : "text-amber"}>
        {lan ? STATE[lan.state] : "—"}{lan?.port ? ` · port ${lan.port}` : ""}
      </dd>
      <dt>Hedef</dt>
      <dd class="break-all">{target ? `${target.name} · ${target.root ?? "bağlı değil"}` : "birincil kütüphane yok"}</dd>
    </dl>
    <p class="text-xs text-ink-2">
      Telefonla QR kodu okutun; açılan sayfadan fotoğraf ve video seçip gönderin. Telefondan gelenler birincil
      kütüphaneye, çekim tarihine göre klasörlere yazılır. Hedefi Kütüphaneler sekmesinde "Birincil yap" ile değiştirin.
    </p>
    <div>
      <button class="btn" onclick={() => (catalog.overlay = "wifi")}
        ><Wifi size={14} strokeWidth={1.5} />QR kodu ve eşleşmiş cihazlar</button
      >
    </div>
  </section>

  <section class="flex flex-col gap-2">
    <h2 class="label">iPhone Kestirmesi (albümü tek dokunuşla)</h2>
    <p class="text-xs text-ink-2">
      Telefondaki Loupe sayfasının altında <b>iPhone Kestirmesi</b> bölümü var: önce sertifika profilini kurun, sonra
      Kestirme anahtarını oluşturup adımları izleyin. Kestirme seçtiğiniz albümün tamamını gönderir; daha önce
      gönderilenler atlanır.
    </p>
  </section>

  <section class="flex flex-col gap-2">
    <h2 class="label">Kabloyla iPhone</h2>
    <p class="text-xs text-ink-2">
      iPhone'u takın, kilidini açıp "Bu bilgisayara güven" deyin, kenar çubuğunda
      <Smartphone size={12} strokeWidth={1.5} class="inline" /> <b>iPhone'dan İçe Aktar</b>'a basın. Açılan pencerede hedef
      kütüphaneyi, alt klasörü (varsayılan <span class="data">iPhone</span>) ve klasör düzenini seçersiniz. Kablo yalnızca
      Kamera klasörünü verir; albümler gelmez. Telefondan hiçbir şey silinmez.
    </p>
  </section>
</div>
