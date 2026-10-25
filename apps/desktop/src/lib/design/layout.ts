/**
 * Loupe — düzen sabitleri (RFC-001B §4.3).
 *
 * app.css içindeki --spacing-* değişkenleriyle birebir aynıdır. Sanal kaydırma
 * ve justified düzen hesabı Web Worker'da yapıldığı için CSS'ten okunamaz; bu
 * yüzden sayılar burada da tutulur. Birini değiştiren diğerini de değiştirir.
 */

/** Fotoğraflar arası oluk (px). */
export const GUTTER = 2;

/** Zaman çizelgesi ay başlığı yüksekliği (px). */
export const SECTION_HEADER = 32;

/** Bölümler arası dikey boşluk (px). */
export const SECTION_GAP = 16;

/** Yakınlaştırma seviyelerine göre hedef satır yüksekliği (CSS px). Ctrl+= / Ctrl+- */
export const ROW_HEIGHTS = [120, 180, 260, 380] as const;
export type ZoomLevel = 0 | 1 | 2 | 3;
export const DEFAULT_ZOOM: ZoomLevel = 1;

/**
 * Bir satırın hedef yükseklikten sapabileceği sınırlar (oran).
 * Tam satırlar [MIN, MAX] aralığında esner; aralık dışına düşen tek
 * panorama kendi satırına yerleşir (RFC-001B §4.4).
 */
export const ROW_STRETCH_MIN = 0.7;
export const ROW_STRETCH_MAX = 1.5;

/** Görünür alanın üstünde ve altında DOM'da tutulan ek alan (ekran yüksekliği katı). */
export const OVERSCAN_SCREENS = 1.5;

/**
 * Bu hızın (ekran/sn) üstünde kaydırırken küçük resim istenmez; yalnızca
 * ThumbHash yer tutucusu gösterilir. Yavaşlayınca istekler başlar.
 */
export const FAST_SCROLL_SCREENS_PER_S = 4;

/** Görünür alan değişikliğinin daemon'a bildirilme aralığı (ms). */
export const VIEWPORT_REPORT_INTERVAL_MS = 100;

/**
 * Küçük resim boyut sınıfları (px, cihaz pikseli).
 * grid: yükseklik; preview/full: uzun kenar. Seçim: gereken = satırYüksekliği × devicePixelRatio;
 * gereken ≤ grid ise grid, değilse preview. full yalnızca QuickLook ve tam ekran.
 */
export const THUMB_PX = { grid: 512, preview: 1600, full: 2560 } as const;
export type ThumbClass = keyof typeof THUMB_PX;

/** Kontrol yüksekliği, araç çubuğu, durum çubuğu, paneller (px). */
export const CONTROL = 28;
export const TOOLBAR = 40;
export const STATUSBAR = 24;
export const SIDEBAR = 240;
export const INSPECTOR = 320;
