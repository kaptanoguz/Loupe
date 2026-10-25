/**
 * Yüz kapağı kırpması: küçük resmi, yüz kutusu (çevresiyle) kabı dolduracak
 * şekilde ölçekler ve kaydırır. Kap hep dolar (boşluk kalmaz); yüz kabın
 * ortasına olabildiğince yakın durur. Kapak kare ya da portredir, asla daire.
 */
import type { FaceRef } from "../api/types";

/** Yüz kutusunun etrafında bırakılan pay (kutu boyutunun katı). */
export const FACE_MARGIN_W = 1.7;
export const FACE_MARGIN_H = 1.9;

export interface Crop {
  /** Görüntünün çizilecek boyutu ve kaba göre konumu (px). */
  width: number;
  height: number;
  left: number;
  top: number;
}

export function faceCrop(face: Pick<FaceRef, "aspect" | "bbox">, boxW: number, boxH: number): Crop {
  const aspect = face.aspect > 0 ? face.aspect : 1;
  const b = face.bbox;
  const bw = Math.max(b.w, 0.01);
  const bh = Math.max(b.h, 0.01);
  // Yüz (paylarıyla) kaba sığsın: iki eksenin küçük ölçeği.
  const byWidth = boxW / (bw * FACE_MARGIN_W);
  const byHeight = (boxH / (bh * FACE_MARGIN_H)) * aspect;
  // Görüntü kabı her iki eksende de örtmeli.
  const width = Math.max(Math.min(byWidth, byHeight), boxW, boxH * aspect);
  const height = width / aspect;
  const cx = (b.x + bw / 2) * width;
  const cy = (b.y + bh / 2) * height;
  const left = Math.min(0, Math.max(boxW - width, boxW / 2 - cx));
  const top = Math.min(0, Math.max(boxH - height, boxH / 2 - cy));
  return { width, height, left, top };
}
