/**
 * Klavye haritası (RFC-001B §4.6). Saf: tuş olayı → eylem. Durum değişikliği
 * `Catalog.dispatch`'tedir; böylece kısayollar DOM'suz test edilir.
 */
import type { ColorLabel, CullRequest, TileItem } from "../api/types";
import type { Direction } from "../layout/virtual";

export type Action =
  | { type: "move"; dir: Direction }
  | { type: "rate"; value: 0 | 1 | 2 | 3 | 4 | 5 }
  | { type: "label"; label: ColorLabel }
  | { type: "favorite" }
  | { type: "inspector" }
  | { type: "loupe" }
  | { type: "loupeZoom" }
  | { type: "close" }
  | { type: "zoom"; delta: 1 | -1 }
  | { type: "palette" };

/** 6–9: kırmızı, sarı, yeşil, mavi (Lightroom sırası). */
export const DIGIT_LABELS: Record<string, ColorLabel> = { "6": "red", "7": "yellow", "8": "green", "9": "blue" };

export interface KeyLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

export interface KeyContext {
  loupeOpen: boolean;
}

const ARROWS: Record<string, Direction> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down" };

export function keyToAction(e: KeyLike, ctx: KeyContext): Action | null {
  const mod = e.ctrlKey || e.metaKey;
  if (mod) {
    if (e.key === "=" || e.key === "+") return { type: "zoom", delta: 1 };
    if (e.key === "-") return { type: "zoom", delta: -1 };
    if (e.key === "k" || e.key === "K") return { type: "palette" };
    return null;
  }
  if (e.altKey) return null;
  const dir = ARROWS[e.key];
  if (dir) {
    // Loupe'da yalnızca önceki/sonraki.
    if (ctx.loupeOpen && (dir === "up" || dir === "down")) return null;
    return { type: "move", dir };
  }
  if (/^[0-5]$/.test(e.key)) return { type: "rate", value: Number(e.key) as 0 | 1 | 2 | 3 | 4 | 5 };
  const label = DIGIT_LABELS[e.key];
  if (label) return { type: "label", label };
  switch (e.key) {
    case " ":
      return { type: "loupe" };
    case "Enter":
      return ctx.loupeOpen ? null : { type: "loupe" };
    case "Escape":
      return { type: "close" };
    case "f":
    case "F":
      return { type: "favorite" };
    case "i":
    case "I":
      return { type: "inspector" };
    case "z":
    case "Z":
      return ctx.loupeOpen ? { type: "loupeZoom" } : null;
    default:
      return null;
  }
}

/** Kürasyon eylemini seçili öğe için çekirdek isteğine çevirir (etiket ve favori aç/kapa). */
export function cullRequestFor(action: Action, item: TileItem): CullRequest | null {
  switch (action.type) {
    case "rate":
      return { ids: [item.id], rating: action.value };
    case "label":
      return { ids: [item.id], label: item.label === action.label ? "none" : action.label };
    case "favorite":
      return { ids: [item.id], favorite: !item.favorite };
    default:
      return null;
  }
}

/** Odak bir metin alanındaysa kısayollar devre dışı. */
export function isTypingTarget(t: EventTarget | null): boolean {
  if (typeof HTMLElement === "undefined" || !(t instanceof HTMLElement)) return false;
  return t.isContentEditable || t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT";
}
