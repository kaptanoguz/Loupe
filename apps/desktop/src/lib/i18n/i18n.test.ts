import { afterEach, describe, expect, it } from "vitest";

import { en } from "./en";
import { coreText, t, useLang } from "./index";
import { tr } from "./tr";

afterEach(() => useLang("tr"));

const holes = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();

describe("i18n", () => {
  it("her anahtarın iki dilde de metni ve aynı yer tutucuları var", () => {
    for (const key of Object.keys(tr) as (keyof typeof tr)[]) {
      expect(en[key], key).toBeTruthy();
      // Biçimlerin her biri aynı yer tutucuları taşır (`{yyyy}` gibi düz metinler de).
      for (const form of en[key].split("|")) expect(holes(form), key).toEqual(holes(tr[key]));
    }
  });

  it("İngilizcede sayıya göre tekil/çoğul seçer; biçimlenmiş sayılar da okunur", () => {
    useLang("en");
    expect(t("status.photos", { n: 1 })).toBe("1 photo");
    expect(t("status.photos", { n: "1" })).toBe("1 photo");
    expect(t("status.photos", { n: "104 233" })).toBe("104 233 photos");
    expect(t("view.all")).toBe("All Photos");
    useLang("tr");
    expect(t("status.photos", { n: 1 })).toBe("1 fotoğraf");
  });

  it("çekirdeğin Türkçe tarih ve bölüm başlıkları İngilizcede çevrilir, adlar korunur", () => {
    useLang("en");
    expect(coreText("24 Ekim 2026")).toBe("24 October 2026");
    expect(coreText("Cumartesi")).toBe("Saturday");
    expect(coreText("Cuma")).toBe("Friday");
    expect(coreText("12–15 Mart 2026")).toBe("12–15 March 2026");
    expect(coreText("Aralık 2025 – Ocak 2026")).toBe("December 2025 – January 2026");
    expect(coreText("3 yıldız")).toBe("3 stars");
    expect(coreText("1 yıldız")).toBe("1 star");
    expect(coreText("Puansız")).toBe("Unrated");
    expect(coreText("Ada göre")).toBe("By name");
    expect(coreText("Martin")).toBe("Martin");
    expect(coreText(null)).toBeNull();
    useLang("tr");
    expect(coreText("24 Ekim 2026")).toBe("24 Ekim 2026");
  });
});
