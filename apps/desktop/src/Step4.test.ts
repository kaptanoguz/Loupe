import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

// Telefon sayfası (crates/loupe-lan/web) bağımlılıksız düz JS; burada aynı dosyalar sınanır.
// @ts-expect-error -- tür bildirimi olmayan düz JS modülü
import { Blake3, blake3Hex, toHex } from "../../../crates/loupe-lan/web/blake3.js";
import {
  RateMeter,
  assetId,
  deviceNameFromUA,
  formatBytes,
  formatClock,
  formatRate,
  parseFragment,
  platformFromUA,
  retryDelay,
  splitByPrecheck,
  tusMetadata,
  // @ts-expect-error -- tür bildirimi olmayan düz JS modülü
} from "../../../crates/loupe-lan/web/core.js";
import App from "./App.svelte";
import { MockBackend } from "./lib/api/mock";
import { LayoutEngine } from "./lib/layout/engine";

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 800 });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      private cb: ResizeObserverCallback;
      constructor(cb: ResizeObserverCallback) {
        this.cb = cb;
      }
      observe(target: Element): void {
        this.cb([{ target, contentRect: { width: 1200, height: 800 } } as unknown as ResizeObserverEntry], this as never);
      }
      unobserve(): void {}
      disconnect(): void {}
    },
  );
});

afterEach(() => cleanup());

/** Resmî BLAKE3 test vektörleri: girdi `i % 251` baytları. */
const VECTORS: Record<number, string> = {
  0: "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
  1: "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
  63: "e9bc37a594daad83be9470df7f7b3798297c3d834ce80ba85d6e207627b7db7b",
  64: "4eed7141ea4a5cd4b788606bd23f46e212af9cacebacdc7d1f4c6dc7f2511b98",
  65: "de1e5fa0be70df6d2be8fffd0e99ceaa8eb6e8c93a63f2d8d1c30ecb6b263dee",
  1023: "10108970eeda3eb932baac1428c7a2163b0e924c9a9e25b35bba72b28f70bd11",
  1024: "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
  1025: "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444",
  2048: "e776b6028c7cd22a4d0ba182a8bf62205d2ef576467e838ed6f2529b85fba24a",
  2049: "5f4d72f40d7a5f82b15ca2b2e44b1de3c2ef86c426c95c1af0b6879522563030",
  3073: "7124b49501012f81cc7f11ca069ec9226cecb8a2c850cfe644e327d22d3e1cd3",
  4097: "9b4052b38f1c5fc8b1f9ff7ac7b27cd242487b3d890d15c96a1c25b8aa0fb995",
  8193: "bab6c09cb8ce8cf459261398d2e7aef35700bf488116ceb94a36d0f5f1b7bc3b",
  31744: "62b6960e1a44bcc1eb1a611a8d6235b6b4b78f32e7abc4fb4c6cdcce94895c47",
  102400: "bc3e3d41a1146b069abffad3c0d44860cf664390afce4d9661f7902e7943e085",
};

const input = (n: number) => Uint8Array.from({ length: n }, (_, i) => i % 251);

describe("telefon sayfası: BLAKE3", () => {
  it("resmî test vektörleri (tek seferde)", () => {
    for (const [n, hex] of Object.entries(VECTORS)) expect(blake3Hex(input(Number(n))), `${n} bayt`).toBe(hex);
  });

  it("parça parça güncelleme ve hizasız görünümler aynı özeti verir", () => {
    const data = input(102400);
    for (const step of [1, 7, 64, 65, 1000, 1024, 4096]) {
      const h = new Blake3();
      for (let i = 0; i < data.length; i += step) h.update(data.subarray(i, i + step));
      expect(toHex(h.digest()), `adım ${step}`).toBe(VECTORS[102400]);
    }
    // byteOffset 4'ün katı değil: sözcük okumaları yavaş yola düşer.
    const buf = new Uint8Array(3073 + 3);
    buf.set(input(3073), 3);
    expect(blake3Hex(buf.subarray(3))).toBe(VECTORS[3073]);
  });
});

describe("telefon sayfası: yardımcılar", () => {
  it("QR parçasından belirteç", () => {
    expect(parseFragment("#t=YVsac3HzNmpz4-oT2fLOPMiQJN0tj0Sn1Y33VnQoF2o").token).toBe(
      "YVsac3HzNmpz4-oT2fLOPMiQJN0tj0Sn1Y33VnQoF2o",
    );
    expect(parseFragment("").token).toBeNull();
    expect(parseFragment("#t=kisa").token).toBeNull();
    expect(parseFragment("#t=<script>alert(1)</script>xxxxxxxxxx").token).toBeNull();
  });

  it("tus üst verisi UTF-8 base64", () => {
    const md = tusMetadata({ filename: "Kız Kulesi ğüşiöç.jpg", modified_at: 1761390000000, resolution: undefined });
    const pairs = Object.fromEntries(
      md.split(",").map((p: string) => {
        const [k, v] = p.split(" ");
        const bin = atob(v ?? "");
        return [k, new TextDecoder().decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)))];
      }),
    );
    expect(pairs).toEqual({ filename: "Kız Kulesi ğüşiöç.jpg", modified_at: "1761390000000" });
    expect(assetId({ name: "IMG_0001.HEIC" })).toBe("web:IMG_0001.HEIC");
  });

  it("platform ve cihaz adı", () => {
    const pixel =
      "Mozilla/5.0 (Linux; Android 14; Pixel 7 Build/AP2A.240805.005) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Mobile Safari/537.36";
    const iphone =
      "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
    const reduced = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Mobile Safari/537.36";
    expect([platformFromUA(pixel), deviceNameFromUA(pixel)]).toEqual(["android", "Pixel 7 (Chrome)"]);
    expect([platformFromUA(iphone), deviceNameFromUA(iphone)]).toEqual(["ios", "iPhone (Safari)"]);
    expect(deviceNameFromUA(reduced)).toBe("Android telefon (Chrome)");
    expect(platformFromUA("Mozilla/5.0 (X11; Linux x86_64) Firefox/131.0")).toBe("desktop");
    expect(deviceNameFromUA("x".repeat(300)).length).toBeLessThanOrEqual(80);
  });

  it("biçimler, hız ölçer, yeniden deneme ve ön kontrol ayrımı", () => {
    expect([formatBytes(0), formatBytes(15_300_000), formatBytes(2_500_000_000)]).toEqual(["0 B", "15.3 MB", "2.50 GB"]);
    expect([formatRate(0), formatRate(12_100_000)]).toEqual(["—", "12.1 MB/s"]);
    expect([formatClock(61), formatClock(3725), formatClock(-1)]).toEqual(["1:01", "1:02:05", "—"]);
    const m = new RateMeter(3000);
    m.add(10_000_000, 0);
    m.add(10_000_000, 1000);
    expect(m.rate(2000)).toBeCloseTo(10_000_000, -3);
    expect(m.rate(10_000)).toBe(0);
    expect(retryDelay(0)).toBe(1000);
    expect(retryDelay(100)).toBeNull();
    const a = { name: "a.jpg" };
    const b = { name: "b.jpg" };
    expect(splitByPrecheck([a, b], ["web:b.jpg"])).toEqual({ send: [a], skip: [b] });
  });
});

describe("Ayarlar → Yapay zekâ → Modeller", () => {
  it("eksik modeller listelenir, indirme ilerler, bitince yeniden başlatma istenir", async () => {
    const backend = new MockBackend(200);
    const { container } = render(App, { backend, engine: new LayoutEngine(false) });
    await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
    await fireEvent.click(screen.getByLabelText("Ayarlar"));
    const dialog = screen.getByRole("dialog", { name: "Ayarlar" });
    await fireEvent.click(within(dialog).getByRole("tab", { name: "Yapay zekâ" }));
    const section = await within(dialog).findByLabelText("Modeller");
    await waitFor(() => expect(within(section).getAllByText("yok").length).toBe(6));
    expect(within(section).getByText("Yüz tespiti · SCRFD")).toBeTruthy();
    expect(within(section).getByText(/ticari olmayan/)).toBeTruthy();

    const install = vi.spyOn(backend, "modelsInstall");
    await fireEvent.click(within(section).getByText(/Modelleri indir/));
    expect(install).toHaveBeenCalled();
    await waitFor(() => expect(within(section).getByText("İptal")).toBeTruthy());
    await waitFor(() => expect(within(section).getAllByText("doğrulandı").length).toBe(6), { timeout: 8000 });
    expect(within(section).getByText(/yeniden başlatın/)).toBeTruthy();
    expect(within(section).queryByText(/Modelleri indir/)).toBeNull();
  }, 15000);
});

describe("Wi-Fi: tarayıcıyla eşleşme", () => {
  it("QR kamera ile açılır; sertifika parmak izi ve tarayıcı cihazı görünür", async () => {
    const backend = new MockBackend(200);
    const { container } = render(App, { backend, engine: new LayoutEngine(false) });
    await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
    expect((await backend.pairingQr()).uri).toMatch(/^https:\/\/192\.168\.1\.20:\d+\/m#t=/);
    await fireEvent.click(screen.getByTitle("Wi-Fi Telefon Eşleştir"));
    const dialog = screen.getByRole("dialog", { name: "Wi-Fi telefon eşleştirme" });
    expect(within(dialog).getByText(/uygulama kurmak gerekmez/)).toBeTruthy();
    const fp = await within(dialog).findByLabelText("Sertifika parmak izi");
    expect(fp.textContent?.split(":").length).toBe(32);
    await waitFor(() => expect(within(dialog).getByText("android · tarayıcı")).toBeTruthy());
  });
});
