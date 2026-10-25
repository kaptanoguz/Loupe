/**
 * Renk etiketi sınıfları. Tailwind sınıfları kaynakta tam yazılmalı
 * (dinamik `bg-label-${x}` taranamaz).
 */
import type { ColorLabel } from "../api/types";

export const LABEL_BG: Record<ColorLabel, string> = {
  red: "bg-label-red",
  yellow: "bg-label-yellow",
  green: "bg-label-green",
  blue: "bg-label-blue",
  purple: "bg-label-purple",
};

export const LABEL_NAME: Record<ColorLabel, string> = {
  red: "Kırmızı",
  yellow: "Sarı",
  green: "Yeşil",
  blue: "Mavi",
  purple: "Mor",
};
