import { describe, expect, it } from "vitest";

import type { TileItem } from "../api/types";
import { cullRequestFor, keyToAction } from "./keymap";

const grid = { loupeOpen: false };
const loupe = { loupeOpen: true };

const item: TileItem = {
  id: 9,
  hash: "ab".repeat(32),
  aspect: 1.5,
  rating: 2,
  label: "red",
  favorite: false,
  flag: "none",
  raw: null,
  video: false,
  durationMs: null,
  sync: "local",
  takenAtUtc: 0,
  tzOffsetMin: null,
};

describe("keyToAction", () => {
  it("oklar", () => {
    expect(keyToAction({ key: "ArrowLeft" }, grid)).toEqual({ type: "move", dir: "left" });
    expect(keyToAction({ key: "ArrowDown" }, grid)).toEqual({ type: "move", dir: "down" });
    // Loupe'da ↑/↓ yok, ←/→ önceki/sonraki.
    expect(keyToAction({ key: "ArrowDown" }, loupe)).toBeNull();
    expect(keyToAction({ key: "ArrowRight" }, loupe)).toEqual({ type: "move", dir: "right" });
  });

  it("0–5 puan, 6–9 renk etiketi", () => {
    expect(keyToAction({ key: "0" }, grid)).toEqual({ type: "rate", value: 0 });
    expect(keyToAction({ key: "5" }, grid)).toEqual({ type: "rate", value: 5 });
    expect(keyToAction({ key: "6" }, grid)).toEqual({ type: "label", label: "red" });
    expect(keyToAction({ key: "7" }, grid)).toEqual({ type: "label", label: "yellow" });
    expect(keyToAction({ key: "8" }, grid)).toEqual({ type: "label", label: "green" });
    expect(keyToAction({ key: "9" }, grid)).toEqual({ type: "label", label: "blue" });
  });

  it("F, I, Space, Esc, Z", () => {
    expect(keyToAction({ key: "f" }, grid)).toEqual({ type: "favorite" });
    expect(keyToAction({ key: "s" }, grid)).toEqual({ type: "slideshow" });
    expect(keyToAction({ key: "I" }, grid)).toEqual({ type: "inspector" });
    expect(keyToAction({ key: " " }, grid)).toEqual({ type: "loupe" });
    expect(keyToAction({ key: " " }, loupe)).toEqual({ type: "loupe" });
    expect(keyToAction({ key: "Enter" }, grid)).toEqual({ type: "loupe" });
    expect(keyToAction({ key: "Escape" }, loupe)).toEqual({ type: "close" });
    expect(keyToAction({ key: "z" }, grid)).toBeNull();
    expect(keyToAction({ key: "z" }, loupe)).toEqual({ type: "loupeZoom" });
  });

  it("Ctrl+=/− yakınlaştırma; diğer değiştiriciler yutulmaz", () => {
    expect(keyToAction({ key: "=", ctrlKey: true }, grid)).toEqual({ type: "zoom", delta: 1 });
    expect(keyToAction({ key: "-", ctrlKey: true }, grid)).toEqual({ type: "zoom", delta: -1 });
    expect(keyToAction({ key: "1", ctrlKey: true }, grid)).toBeNull();
    expect(keyToAction({ key: "k", ctrlKey: true }, grid)).toEqual({ type: "palette" });
    expect(keyToAction({ key: "k", metaKey: true }, grid)).toEqual({ type: "palette" });
    expect(keyToAction({ key: "k" }, grid)).toBeNull();
    expect(keyToAction({ key: "f", altKey: true }, grid)).toBeNull();
    expect(keyToAction({ key: "q" }, grid)).toBeNull();
  });
});

describe("cullRequestFor", () => {
  it("puan doğrudan yazılır", () => {
    expect(cullRequestFor({ type: "rate", value: 0 }, item)).toEqual({ ids: [9], rating: 0 });
  });
  it("aynı etiket kaldırır, farklı etiket değiştirir", () => {
    expect(cullRequestFor({ type: "label", label: "red" }, item)).toEqual({ ids: [9], label: "none" });
    expect(cullRequestFor({ type: "label", label: "blue" }, item)).toEqual({ ids: [9], label: "blue" });
  });
  it("favori aç/kapa", () => {
    expect(cullRequestFor({ type: "favorite" }, item)).toEqual({ ids: [9], favorite: true });
    expect(cullRequestFor({ type: "favorite" }, { ...item, favorite: true })).toEqual({ ids: [9], favorite: false });
  });
  it("kürasyon dışı eylem istek üretmez", () => {
    expect(cullRequestFor({ type: "inspector" }, item)).toBeNull();
  });
});
