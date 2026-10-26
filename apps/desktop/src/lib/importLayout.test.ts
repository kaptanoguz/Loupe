import { describe, expect, it } from "vitest";

import { joinLayout, layoutError, previewPath, splitLayout } from "./importLayout";

describe("importLayout", () => {
  it("alt klasör ve tarih desenine ayırır, geri birleştirir", () => {
    expect(splitLayout("iPhone/{yyyy}/{mm}")).toEqual({ sub: "iPhone", date: "{yyyy}/{mm}" });
    expect(splitLayout("{yyyy}/{mm}/{dd}")).toEqual({ sub: "", date: "{yyyy}/{mm}/{dd}" });
    expect(splitLayout("Telefon/Ayşe")).toEqual({ sub: "Telefon/Ayşe", date: "" });
    expect(joinLayout(" /iPhone/ ", "{yyyy}")).toBe("iPhone/{yyyy}");
    expect(joinLayout("", "{yyyy}/{mm}")).toBe("{yyyy}/{mm}");
  });

  it("geçersiz düzeni nedeniyle reddeder", () => {
    expect(layoutError("iPhone/{yyyy}")).toBeNull();
    expect(layoutError("")).not.toBeNull();
    expect(layoutError(".gizli/{yyyy}")).not.toBeNull();
    expect(layoutError("../{yyyy}")).not.toBeNull();
    expect(layoutError("a//b")).not.toBeNull();
    expect(layoutError("{hh}")).not.toBeNull();
  });

  it("örnek hedef yolu üretir", () => {
    expect(previewPath("/media/Data/Pictures/", "iPhone/{yyyy}/{mm}/{dd}", "IMG_1234.HEIC")).toBe(
      "/media/Data/Pictures/iPhone/2025/08/14/IMG_1234.HEIC",
    );
  });
});
