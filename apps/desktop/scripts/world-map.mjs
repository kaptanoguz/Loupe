// Yerler haritasının çevrimdışı katmanını üretir: src/lib/map/world.json.
//
// Kaynak: Natural Earth 1:50m ülkeleri (kamu malı), world-atlas@2.0.2 (ISC)
// paketindeki TopoJSON. Kara tek yol, ülke sınırları ikinci yol olarak
// Web Mercator'a izdüşürülür ve `size` birimlik kareye tam sayı göreli
// komutlarla yazılır.
//
// Kullanım (paketler projeye eklenmez):
//   mkdir /tmp/wm && cd /tmp/wm && npm i world-atlas@2.0.2 topojson-client@3.1.0
//   node apps/desktop/scripts/world-map.mjs /tmp/wm apps/desktop/src/lib/map/world.json
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
const require = createRequire(process.argv[2] + "/");
const topo = require("topojson-client");
const W = 16384;
const world = JSON.parse(readFileSync(process.argv[2] + "/node_modules/world-atlas/countries-50m.json", "utf8"));
const land = topo.merge(world, world.objects.countries.geometries);
const borders = topo.mesh(world, world.objects.countries, (a, b) => a !== b);
const MAXLAT = 85.05112878;
const proj = ([lon, lat]) => {
  const l = Math.max(-MAXLAT, Math.min(MAXLAT, lat));
  const x = ((lon + 180) / 360) * W;
  const y = ((1 - Math.log(Math.tan(Math.PI / 4 + (l * Math.PI) / 360)) / Math.PI) / 2) * W;
  return [Math.round(x), Math.round(y)];
};
// Göreli komutlar: kısa sayılar, aynı ayrıntı.
const num = (a, b) => (b < 0 ? `${a}${b}` : `${a} ${b}`);
function ring(coords, close) {
  let out = "", prev = null, n = 0;
  for (const c of coords) {
    const p = proj(c);
    if (prev && p[0] === prev[0] && p[1] === prev[1]) continue;
    out += prev ? (n === 1 ? "l" : " ") + num(p[0] - prev[0], p[1] - prev[1]) : "M" + num(p[0], p[1]);
    prev = p; n++;
  }
  return n > (close ? 2 : 1) ? out + (close ? "Z" : "") : "";
}
// Değişim çizgisini geçen halkalar boylamı sürekli kılınarak açılır ve bir
// kopyası 360° kaydırılır; aksi halde dünyayı boydan boya kesen şeritler çıkar.
function unwrap(r) {
  let off = 0, prev = null, wrapped = false;
  const out = r.map(([lon, lat]) => {
    if (prev !== null) {
      const d = lon + off - prev;
      if (d > 180) { off -= 360; wrapped = true; }
      else if (d < -180) { off += 360; wrapped = true; }
    }
    prev = lon + off;
    return [lon + off, lat];
  });
  return { out, wrapped };
}
let landPath = "";
let wraps = 0;
for (const poly of land.coordinates) for (const r of poly) {
  const { out, wrapped } = unwrap(r);
  const copies = wrapped ? [0, out.some(([l]) => l > 180) ? -360 : 360] : [0];
  if (wrapped) wraps++;
  for (const shift of copies) {
    const s = ring(out.map(([l, a]) => [l + shift, a]), true);
    if (s) landPath += s;
  }
}
console.error("wrapped rings", wraps);
let borderPath = "";
for (const line of borders.coordinates) borderPath += ring(line, false);
writeFileSync(process.argv[3], JSON.stringify({ size: W, land: landPath, borders: borderPath }));
