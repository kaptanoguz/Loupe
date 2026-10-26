/**
 * Renk etiketi sınıfları. Tailwind sınıfları kaynakta tam yazılmalı
 * (dinamik `bg-label-${x}` taranamaz).
 */
import type { ColorLabel } from "../api/types";
import { t } from "../i18n";

export const LABEL_BG: Record<ColorLabel, string> = {
  red: "bg-label-red",
  yellow: "bg-label-yellow",
  green: "bg-label-green",
  blue: "bg-label-blue",
  purple: "bg-label-purple",
};

export const LABEL_NAME: Record<ColorLabel, string> = {
  red: t("label.red"),
  yellow: t("label.yellow"),
  green: t("label.green"),
  blue: t("label.blue"),
  purple: t("label.purple"),
};
