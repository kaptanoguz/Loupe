import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import App from "./App.svelte";
import { MockBackend } from "./lib/api/mock";
import type { PersonCard } from "./lib/api/types";
import { matchCommand } from "./lib/components/Palette.svelte";
import { mergePlan } from "./lib/components/People.svelte";
import { parseMbps } from "./lib/components/settings/CloudSettings.svelte";
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

function setup(n = 400) {
  const backend = new MockBackend(n);
  const utils = render(App, { backend, engine: new LayoutEngine(false) });
  return { backend, ...utils };
}

async function ready(container: HTMLElement): Promise<void> {
  await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
}

function card(personId: number | null, clusterId: number, photoCount: number, faceCount = photoCount): PersonCard {
  return { personId, clusterId, name: personId === null ? null : `K${personId}`, faceCount, photoCount, cover: null };
}

describe("saf yardımcılar", () => {
  it("birleştirme planı: hedef en çok fotoğraflı kişi, isimsizler adıyla katılır", () => {
    expect(mergePlan([card(1, 10, 5)])).toBeNull();
    expect(mergePlan([card(1, 10, 5), card(2, 20, 9), card(null, 30, 3)])).toEqual({
      kind: "persons",
      into: 2,
      from: [1],
      clusters: [30],
    });
    expect(mergePlan([card(null, 30, 3, 3), card(null, 40, 2, 8)])).toEqual({ kind: "clusters", into: 40, from: [30] });
  });

  it("komut eşleşmesi Türkçe büyük/küçük harfe duyarsız", () => {
    const cmd = { id: "verify" as const, label: "Kütüphaneyi Doğrula (Bit-Rot)", hint: "BLAKE3" };
    expect(matchCommand(cmd, "")).toBe(true);
    expect(matchCommand(cmd, "DOĞRULA")).toBe(true);
    expect(matchCommand(cmd, "bit rot")).toBe(true);
    expect(matchCommand(cmd, "wifi")).toBe(false);
  });

  it("MB/s girdisi", () => {
    expect(parseMbps("")).toBeNull();
    expect(parseMbps("4,5")).toBe(4.5);
    expect(parseMbps("-1")).toBeUndefined();
    expect(parseMbps("abc")).toBeUndefined();
  });
});

describe("komut paleti", () => {
  it("Ctrl+K açar; kişi önerisi Tab ile alınır; sonuçlar ızgarada açılır ve akıllı albüm kaydedilir", async () => {
    const { container, backend } = setup();
    await ready(container);
    await fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    const dialog = screen.getByRole("dialog", { name: "Komut paleti" });
    // Düz yüzey: gölge/blur sınıfı yok.
    expect(dialog.className).toContain("palette");
    expect(dialog.className).not.toMatch(/shadow|blur/);
    const input = within(dialog).getByRole("combobox");
    // Boş girdide hızlı komutlar listelenir.
    await waitFor(() => expect(within(dialog).getByText("Kütüphaneyi Doğrula (Bit-Rot)")).toBeTruthy());
    expect(within(dialog).getByText("Wi-Fi Telefon Eşleştir")).toBeTruthy();
    expect(within(dialog).getByText("Bulut Senkronizasyonunu Başlat")).toBeTruthy();

    await fireEvent.input(input, { target: { value: "person:ay" } });
    const list = within(dialog).getByRole("listbox");
    await waitFor(() => expect(within(list).getByText("Ayşe Yılmaz")).toBeTruthy());
    await fireEvent.keyDown(input, { key: "Tab" });
    expect((input as HTMLInputElement).value).toBe('person:"Ayşe Yılmaz" ');
    await waitFor(() => expect(within(dialog).getByText("Tüm sonuçları ızgarada aç")).toBeTruthy());
    expect(within(dialog).getByLabelText("Süzgeçler").textContent).toContain("Ayşe Yılmaz");

    await fireEvent.click(within(dialog).getByText("Tüm sonuçları ızgarada aç"));
    expect(screen.queryByRole("dialog", { name: "Komut paleti" })).toBeNull();
    const header = screen.getByRole("banner", { name: "Görünüm başlığı" });
    expect(header.textContent).toContain("Ayşe Yılmaz");

    const save = vi.spyOn(backend, "saveSmartAlbum");
    await fireEvent.click(within(header).getByText("Akıllı albüm olarak kaydet"));
    const name = within(header).getByLabelText("Akıllı albüm adı") as HTMLInputElement;
    await fireEvent.input(name, { target: { value: "Ayşe" } });
    await fireEvent.keyDown(name, { key: "Enter" });
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0]?.[1]).toEqual({ persons: { ids: [1], mode: "all" } });
    const nav = screen.getByRole("navigation", { name: "Kenar çubuğu" });
    await waitFor(() => expect(within(nav).getByText("Ayşe")).toBeTruthy());
  });

  it("hatalı belirteç kehribar çip olur ve arama yapılmaz", async () => {
    const { container } = setup();
    await ready(container);
    await fireEvent.keyDown(window, { key: "K", metaKey: true });
    const dialog = screen.getByRole("dialog", { name: "Komut paleti" });
    await fireEvent.input(within(dialog).getByRole("combobox"), { target: { value: "rating:9 " } });
    await waitFor(() => expect(within(dialog).getByText(/1–5 arası/)).toBeTruthy());
    expect(within(dialog).queryByText("Tüm sonuçları ızgarada aç")).toBeNull();
    await fireEvent.keyDown(within(dialog).getByRole("combobox"), { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Komut paleti" })).toBeNull();
  });

  it("komut: Bit-Rot doğrulaması başlar ve rapor Ayarlar'da görünür", async () => {
    const { container } = setup();
    await ready(container);
    await fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    const dialog = screen.getByRole("dialog", { name: "Komut paleti" });
    const input = within(dialog).getByRole("combobox");
    await fireEvent.input(input, { target: { value: "bit-rot" } });
    await waitFor(() => expect(within(dialog).getAllByRole("option")).toHaveLength(1));
    await fireEvent.keyDown(input, { key: "Enter" });
    const settings = await screen.findByRole("dialog", { name: "Ayarlar" });
    const report = await within(settings).findByLabelText("Son doğrulama raporu");
    expect(report.textContent).toContain("Fotoğraflar");
    expect(report.textContent).toContain("Bozuk");
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Ayarlar" })).toBeNull();
  });
});

describe("Wi-Fi eşleştirme", () => {
  it("sol alttan açılır: QR yalnızca rect/path, geri sayım monospace, cihaz politikası ve erişim kaldırma", async () => {
    const { container, backend } = setup();
    await ready(container);
    await fireEvent.click(screen.getByTitle("Wi-Fi Telefon Eşleştir"));
    const dialog = screen.getByRole("dialog", { name: "Wi-Fi telefon eşleştirme" });
    const svg = await within(dialog).findByRole("img", { name: "Eşleşme QR kodu" });
    expect(svg.querySelectorAll("circle").length).toBe(0);
    const shapes = new Set([...svg.querySelectorAll("*")].map((el) => el.tagName.toLowerCase()));
    expect([...shapes].sort()).toEqual(["path", "rect"]);
    const countdown = within(dialog).getByLabelText("Kalan süre");
    expect(countdown.className).toContain("font-mono");
    expect(countdown.textContent?.trim()).toMatch(/^[01]:\d\d$/);

    await waitFor(() => expect(within(dialog).getByText("Oğuz'un telefonu")).toBeTruthy());
    const policy = vi.spyOn(backend, "setConflictPolicy");
    const select = within(dialog).getByLabelText("Oğuz'un telefonu çakışma politikası") as HTMLSelectElement;
    expect(select.value).toBe("keep_both");
    await fireEvent.change(select, { target: { value: "replace_with_newer" } });
    await waitFor(() => expect(policy).toHaveBeenCalledWith(1, "replace_with_newer"));

    await fireEvent.click(within(dialog).getByText("Erişimi kaldır"));
    await fireEvent.click(within(dialog).getByText("Kaldır"));
    await waitFor(() => expect(within(dialog).getByText(/erişim kaldırıldı/)).toBeTruthy());

    const cancel = vi.spyOn(backend, "cancelPairing");
    await fireEvent.click(within(dialog).getByLabelText("Kapat"));
    expect(screen.queryByRole("dialog", { name: "Wi-Fi telefon eşleştirme" })).toBeNull();
    expect(cancel).toHaveBeenCalled();
  });
});

describe("Kişiler", () => {
  it("kapaklar 2px kare/portre; isimsiz küme adlandırılır, iki kişi birleştirilir", async () => {
    const { container } = setup();
    await ready(container);
    const nav = screen.getByRole("navigation", { name: "Kenar çubuğu" });
    await fireEvent.click(within(nav).getByText("Kişiler"));
    const page = screen.getByRole("region", { name: "Kişiler" });
    await waitFor(() => expect(within(page).getByText("Ayşe Yılmaz")).toBeTruthy());
    expect(container.querySelectorAll("circle").length).toBe(0);
    expect(container.innerHTML).not.toMatch(/rounded-(full|md|lg|xl)/);
    expect(page.querySelector(".rounded-xs")).toBeTruthy();

    // İsimsiz kümeyi seç ve adlandır.
    const unnamed = within(page).getAllByText("İsimsiz", { selector: "button span" })[0] as HTMLElement;
    await fireEvent.click(unnamed.closest("button") as HTMLElement);
    const panel = screen.getByRole("complementary", { name: "Kişi ayrıntısı" });
    const name = within(panel).getByLabelText("Kişinin adı");
    await fireEvent.input(name, { target: { value: "Zeynep" } });
    await fireEvent.keyDown(name, { key: "Enter" });
    await waitFor(() => expect(within(page).getAllByText("Zeynep").length).toBeGreaterThan(0));

    // Ctrl+tıklama ile iki kişi seç, birleştir.
    const ayse = within(page).getByText("Ayşe Yılmaz").closest("button") as HTMLElement;
    const mehmet = within(page).getByText("Mehmet Kaya").closest("button") as HTMLElement;
    await fireEvent.click(ayse);
    await fireEvent.click(mehmet, { ctrlKey: true });
    await fireEvent.click(within(page).getByText("Birleştir"));
    await waitFor(() => expect(within(page).queryAllByText("Mehmet Kaya").length + within(page).queryAllByText("Ayşe Yılmaz").length).toBe(1));
  });

  it("yüz başka kümeye ayrılır; çift tıklama kişinin fotoğraflarını açar", async () => {
    const { container, backend } = setup();
    await ready(container);
    await fireEvent.click(within(screen.getByRole("navigation", { name: "Kenar çubuğu" })).getByText("Kişiler"));
    const page = screen.getByRole("region", { name: "Kişiler" });
    const ayse = (await within(page).findByText("Ayşe Yılmaz")).closest("button") as HTMLElement;
    await fireEvent.click(ayse);
    const panel = screen.getByRole("complementary", { name: "Kişi ayrıntısı" });
    const faces = await within(panel).findAllByRole("button", { name: /^Yüz \d+$/ });
    await fireEvent.click(faces[0] as HTMLElement);
    const detach = vi.spyOn(backend, "detachFace");
    await fireEvent.click(within(panel).getByText("Ayır"));
    await waitFor(() => expect(detach).toHaveBeenCalled());

    await fireEvent.dblClick(ayse);
    await waitFor(() => expect(screen.getByRole("banner", { name: "Görünüm başlığı" }).textContent).toContain("Ayşe Yılmaz"));
    await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
  });
});

describe("Etkinlikler", () => {
  it("yeniden adlandırma kilitler; etkinlik açılır ve fotoğraf çıkarılır", async () => {
    const { container, backend } = setup();
    await ready(container);
    await fireEvent.click(within(screen.getByRole("navigation", { name: "Kenar çubuğu" })).getByText("Etkinlikler"));
    const page = screen.getByRole("region", { name: "Etkinlikler" });
    const first = (await within(page).findAllByLabelText("Yeniden adlandır"))[0] as HTMLElement;
    await fireEvent.click(first);
    const field = within(page).getByLabelText("Etkinlik başlığı");
    await fireEvent.input(field, { target: { value: "Ankara gezisi" } });
    await fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect(within(page).getByText("Ankara gezisi")).toBeTruthy());
    const item = within(page).getByText("Ankara gezisi").closest("li") as HTMLElement;
    expect(within(item).getByLabelText("Kilidi aç").getAttribute("aria-pressed")).toBe("true");

    await fireEvent.click(within(page).getByText("Ankara gezisi"));
    const header = await screen.findByRole("banner", { name: "Görünüm başlığı" });
    await waitFor(() => expect(container.querySelector(".tile.selected")).toBeTruthy());
    const exclude = vi.spyOn(backend, "excludeFromEvent");
    const before = Number(header.querySelector(".data")?.textContent);
    await fireEvent.click(within(header).getByText("Seçili fotoğrafı çıkar"));
    await waitFor(() => expect(exclude).toHaveBeenCalled());
    await waitFor(() => expect(Number(header.querySelector(".data")?.textContent)).toBe(before - 1));
  });
});

describe("Ayarlar", () => {
  it("kurtarma anahtarı: oluştur, göster, üç grubu geri yaz, kur, doğrula", async () => {
    const { container, backend } = setup();
    await ready(container);
    await fireEvent.click(screen.getByLabelText("Ayarlar"));
    const dialog = screen.getByRole("dialog", { name: "Ayarlar" });
    await fireEvent.click(within(dialog).getByRole("tab", { name: "Şifreleme" }));
    await fireEvent.click(await within(dialog).findByText("Kurtarma anahtarı oluştur"));
    const groups = await within(dialog).findByLabelText("Anahtar grupları");
    const shown = [...groups.querySelectorAll("li")].map((li) => li.lastElementChild?.textContent ?? "");
    expect(shown.length).toBeGreaterThan(10);
    await fireEvent.click(within(dialog).getByText("Yazdım, devam"));
    const confirm = within(dialog).getByText("Anahtarı kur") as HTMLButtonElement;
    expect(confirm.disabled).toBe(true);
    for (const input of within(dialog).getAllByLabelText(/^Grup \d+$/)) {
      const g = Number(input.getAttribute("aria-label")?.split(" ")[1]) - 1;
      await fireEvent.input(input, { target: { value: (shown[g] ?? "").toLowerCase() } });
    }
    expect(confirm.disabled).toBe(false);
    await fireEvent.click(confirm);
    await waitFor(() => expect(within(dialog).getByText("Açık anahtar")).toBeTruthy());

    const secret = (await backend.cloudSettings()).encryption.recipient;
    expect(secret).toMatch(/^age1/);
    const check = within(dialog).getByLabelText("Kurtarma anahtarı");
    await fireEvent.input(check, { target: { value: `AGE-SECRET-KEY-1${shown.join("")}` } });
    await fireEvent.click(within(dialog).getByText("Doğrula"));
    await waitFor(() => expect(within(dialog).getByText(/Eşleşiyor/)).toBeTruthy());
  });

  it("kütüphaneler: klasör düzeni, varsayılan mod ve birincil", async () => {
    const { container, backend } = setup();
    const update = vi.spyOn(backend, "updateLibrary");
    await ready(container);
    await fireEvent.click(screen.getByLabelText("Ayarlar"));
    const dialog = screen.getByRole("dialog", { name: "Ayarlar" });
    const lib = within(dialog).getByRole("article", { name: "Fotoğraflar" });
    expect(within(lib).getByText("birincil")).toBeTruthy();
    expect(within(lib).getByText("/home/oguz/Resimler/Loupe/2025/08/14/IMG_1234.HEIC")).toBeTruthy();
    await fireEvent.input(within(lib).getByLabelText("Alt klasör"), { target: { value: ".gizli" } });
    expect(within(lib).getByText(/nokta ile başlayamaz/)).toBeTruthy();
    await fireEvent.input(within(lib).getByLabelText("Alt klasör"), { target: { value: "Arşiv" } });
    await fireEvent.click(within(lib).getByRole("button", { name: "Yıl / Ay" }));
    expect(within(lib).getByText("/home/oguz/Resimler/Loupe/Arşiv/2025/08/IMG_1234.HEIC")).toBeTruthy();
    await fireEvent.click(within(lib).getByRole("button", { name: "Kaydet" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(1, { layoutTemplate: "Arşiv/{yyyy}/{mm}" }));
    await fireEvent.click(within(lib).getByRole("button", { name: "Taşı" }));
    await waitFor(() => expect(update).toHaveBeenCalledWith(1, { defaultImportMode: "move" }));
    // Bağlı olmayan referans kütüphane birincil yapılamaz.
    const ref = within(dialog).getByRole("article", { name: "Arşiv 2019" });
    expect(within(ref).queryByRole("button", { name: "Birincil yap" })).toBeNull();
  });

  it("içe aktarma önce hedefi, düzeni ve modu sorar", async () => {
    const { container, backend } = setup();
    vi.spyOn(backend, "pickImportSource").mockResolvedValue("/media/kart/DCIM");
    const start = vi.spyOn(backend, "startImport");
    await ready(container);
    await fireEvent.click(screen.getByRole("button", { name: "İçe Aktar" }));
    const dialog = await screen.findByRole("dialog", { name: "İçe aktar" });
    expect(start).not.toHaveBeenCalled();
    expect(within(dialog).getByText("/media/kart/DCIM")).toBeTruthy();
    await fireEvent.input(within(dialog).getByLabelText("Alt klasör (isteğe bağlı)"), { target: { value: "Kart" } });
    await fireEvent.click(within(dialog).getByRole("button", { name: "Yıl" }));
    await fireEvent.click(within(dialog).getByRole("button", { name: "Taşı" }));
    expect(within(dialog).getByLabelText("Örnek hedef yol").textContent).toBe("/home/oguz/Resimler/Loupe/Kart/2025/DSC_0412.JPG");
    await fireEvent.click(within(dialog).getByRole("button", { name: "İçe aktar" }));
    await waitFor(() =>
      expect(start).toHaveBeenCalledWith({ libraryId: 1, source: "/media/kart/DCIM", mode: "move", layout: "Kart/{yyyy}" }),
    );
  });

  it("iPhone aktarımı alt klasör önerir ve yalnızca kopyalar", async () => {
    const { container, backend } = setup();
    vi.spyOn(backend, "findIphone").mockResolvedValue([{ udid: "0000-ABC", dcim: "/run/user/1000/gvfs/afc:host=0000-ABC/DCIM" }]);
    const start = vi.spyOn(backend, "startImport");
    await ready(container);
    await fireEvent.click(screen.getByRole("button", { name: /iPhone'dan İçe Aktar/ }));
    const dialog = await screen.findByRole("dialog", { name: "İçe aktar" });
    expect(within(dialog).queryByRole("button", { name: "Taşı" })).toBeNull();
    expect(within(dialog).getByLabelText("Örnek hedef yol").textContent).toBe(
      "/home/oguz/Resimler/Loupe/iPhone/2025/08/14/IMG_1234.HEIC",
    );
    await fireEvent.click(within(dialog).getByRole("button", { name: "İçe aktar" }));
    await waitFor(() => expect(start).toHaveBeenCalledWith(expect.objectContaining({ mode: "copy", layout: "iPhone/{yyyy}/{mm}/{dd}" })));
  });

  it("bulut: sağlayıcıya göre alanlar, MB/s sınırı ve şifreli varsayılan", async () => {
    const { container, backend } = setup();
    await ready(container);
    await fireEvent.click(screen.getByLabelText("Ayarlar"));
    const dialog = screen.getByRole("dialog", { name: "Ayarlar" });
    await fireEvent.click(within(dialog).getByRole("tab", { name: "Bulut" }));
    await waitFor(() => expect(within(dialog).getByText("B2 Yedek")).toBeTruthy());
    expect(within(dialog).getByText(/0600|Secret Service/)).toBeTruthy();
    await fireEvent.click(within(dialog).getByText("+ Hedef ekle"));
    const form = within(dialog).getByLabelText("Hedef ayarı");
    await fireEvent.change(within(form).getByLabelText("Sağlayıcı"), { target: { value: "webdav" } });
    expect(within(form).getByLabelText("Adres")).toBeTruthy();
    expect(within(form).queryByLabelText("Kova")).toBeNull();
    await fireEvent.change(within(form).getByLabelText("Sağlayıcı"), { target: { value: "r2" } });
    await fireEvent.input(within(form).getByLabelText("Hedef adı"), { target: { value: "R2" } });
    await fireEvent.input(within(form).getByLabelText("Uç nokta"), { target: { value: "https://x.r2.cloudflarestorage.com" } });
    await fireEvent.input(within(form).getByLabelText("Kova"), { target: { value: "foto" } });
    await fireEvent.input(within(form).getByLabelText("Erişim anahtarı"), { target: { value: "AKID" } });
    await fireEvent.input(within(form).getByLabelText("Gizli anahtar"), { target: { value: "s3cr3t" } });
    await fireEvent.input(within(form).getByLabelText("Hedef MB/s"), { target: { value: "2,5" } });
    const save = vi.spyOn(backend, "saveCloudTarget");
    await fireEvent.click(within(form).getByText("Kaydet"));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0]).toEqual([
      null,
      {
        name: "R2",
        provider: "r2",
        config: { endpoint: "https://x.r2.cloudflarestorage.com", bucket: "foto" },
        keyPrefix: "loupe/",
        encrypted: true,
        bandwidthMbps: 2.5,
        enabled: true,
        user: "AKID",
        secret: "s3cr3t",
      },
    ]);
    await waitFor(() => expect(within(dialog).getByText("R2")).toBeTruthy());
  });

  it("yapay zekâ: duraklat ve kaynak modu", async () => {
    const { container, backend } = setup();
    await ready(container);
    await waitFor(() => expect(screen.getByRole("contentinfo", { name: "Durum" })).toBeTruthy());
    await fireEvent.click(screen.getByLabelText("Ayarlar"));
    const dialog = screen.getByRole("dialog", { name: "Ayarlar" });
    await fireEvent.click(within(dialog).getByRole("tab", { name: "Yapay zekâ" }));
    const pause = await within(dialog).findByText("Duraklat");
    const spy = vi.spyOn(backend, "setPaused");
    await fireEvent.click(pause);
    await waitFor(() => expect(spy).toHaveBeenCalledWith(true));
    await waitFor(() => expect(within(dialog).getByText("Sürdür")).toBeTruthy());
    const quiet = within(dialog).getByRole("radio", { name: /Sessiz/ });
    await fireEvent.click(quiet);
    await waitFor(() => expect(quiet.getAttribute("aria-checked")).toBe("true"));
  });
});

describe("Loupe 1:1", () => {
  it("1:1'de thumb://orig arka planda yüklenir, lg gelene dek ekranda kalır", async () => {
    const backend = new MockBackend(50);
    vi.spyOn(backend, "thumbUrl").mockImplementation((size, hash) => `thumb://${size}/${hash}`);
    const loads: { src: string; fire: () => void }[] = [];
    const RealImage = globalThis.Image;
    vi.stubGlobal(
      "Image",
      class {
        onload: (() => void) | null = null;
        onerror: (() => void) | null = null;
        decoding = "auto";
        naturalWidth = 6000;
        naturalHeight = 4000;
        set src(v: string) {
          loads.push({ src: v, fire: () => this.onload?.() });
        }
      },
    );
    const { container } = render(App, { backend, engine: new LayoutEngine(false) });
    await ready(container);
    await fireEvent.keyDown(window, { key: " " });
    const loupe = screen.getByRole("dialog", { name: "Loupe" });
    const lg = [...loupe.querySelectorAll("img")].find((i) => i.getAttribute("src")?.startsWith("thumb://lg/"));
    expect(lg).toBeTruthy();
    Object.defineProperty(lg as HTMLImageElement, "naturalWidth", { value: 1920 });
    Object.defineProperty(lg as HTMLImageElement, "naturalHeight", { value: 1280 });
    await fireEvent.load(lg as HTMLImageElement);
    await fireEvent.keyDown(window, { key: "z" });
    await waitFor(() => expect(loads.some((l) => l.src.startsWith("thumb://orig/"))).toBe(true));
    expect(loupe.textContent).toContain("özgün yükleniyor");
    expect(loupe.querySelector("img[src^='thumb://lg/']")).toBeTruthy();
    loads.find((l) => l.src.startsWith("thumb://orig/"))?.fire();
    await waitFor(() => expect(loupe.querySelector("img[src^='thumb://orig/']")).toBeTruthy());
    expect(loupe.textContent).toContain("6000×4000");
    vi.stubGlobal("Image", RealImage);
  });
});
