// BLAKE3 (yalnızca hash kipi, 32 bayt çıktı): bağımlılıksız, akışlı.
// Tarayıcının WebCrypto'sunda BLAKE3 yok; sunucu tus oturumunu dosyanın
// BLAKE3 özetiyle açtığı için telefon özeti kendisi hesaplar.
// Kaynak: BLAKE3 belirtimi ve referans uygulama (CC0 / Apache-2.0).

const IV = new Uint32Array([
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
]);
const CHUNK_START = 1;
const CHUNK_END = 2;
const PARENT = 4;
const ROOT = 8;
const BLOCK_LEN = 64;
const CHUNK_LEN = 1024;
const TWO32 = 0x100000000;
const LITTLE_ENDIAN = new Uint8Array(Uint32Array.of(1).buffer)[0] === 1;

// Yedi turun mesaj sırası (her tur bir öncekine MSG_PERMUTATION uygular).
const PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const SCHEDULE = (() => {
  const out = [];
  let s = Array.from({ length: 16 }, (_, i) => i);
  for (let r = 0; r < 7; r++) {
    out.push(Uint8Array.from(s));
    s = PERM.map((p) => s[p]);
  }
  return out;
})();

/**
 * Sıkıştırma: `m[mo..mo+16]` mesaj sözcükleri; 16 sözcüklük sonucu `out`'a
 * yazar. Durum yerel değişkenlerde tutulur (dizi erişimi yok).
 */
function compress(cv, m, mo, counter, blockLen, flags, out) {
  let v0 = cv[0], v1 = cv[1], v2 = cv[2], v3 = cv[3], v4 = cv[4], v5 = cv[5], v6 = cv[6], v7 = cv[7];
  let v8 = IV[0], v9 = IV[1], v10 = IV[2], v11 = IV[3];
  let v12 = counter % TWO32, v13 = Math.floor(counter / TWO32), v14 = blockLen, v15 = flags;
  for (let r = 0; r < 7; r++) {
    const s = SCHEDULE[r];
    v0 = (v0 + v4 + m[mo + s[0]]) | 0; v12 ^= v0; v12 = (v12 >>> 16) | (v12 << 16);
    v8 = (v8 + v12) | 0; v4 ^= v8; v4 = (v4 >>> 12) | (v4 << 20);
    v0 = (v0 + v4 + m[mo + s[1]]) | 0; v12 ^= v0; v12 = (v12 >>> 8) | (v12 << 24);
    v8 = (v8 + v12) | 0; v4 ^= v8; v4 = (v4 >>> 7) | (v4 << 25);
    v1 = (v1 + v5 + m[mo + s[2]]) | 0; v13 ^= v1; v13 = (v13 >>> 16) | (v13 << 16);
    v9 = (v9 + v13) | 0; v5 ^= v9; v5 = (v5 >>> 12) | (v5 << 20);
    v1 = (v1 + v5 + m[mo + s[3]]) | 0; v13 ^= v1; v13 = (v13 >>> 8) | (v13 << 24);
    v9 = (v9 + v13) | 0; v5 ^= v9; v5 = (v5 >>> 7) | (v5 << 25);
    v2 = (v2 + v6 + m[mo + s[4]]) | 0; v14 ^= v2; v14 = (v14 >>> 16) | (v14 << 16);
    v10 = (v10 + v14) | 0; v6 ^= v10; v6 = (v6 >>> 12) | (v6 << 20);
    v2 = (v2 + v6 + m[mo + s[5]]) | 0; v14 ^= v2; v14 = (v14 >>> 8) | (v14 << 24);
    v10 = (v10 + v14) | 0; v6 ^= v10; v6 = (v6 >>> 7) | (v6 << 25);
    v3 = (v3 + v7 + m[mo + s[6]]) | 0; v15 ^= v3; v15 = (v15 >>> 16) | (v15 << 16);
    v11 = (v11 + v15) | 0; v7 ^= v11; v7 = (v7 >>> 12) | (v7 << 20);
    v3 = (v3 + v7 + m[mo + s[7]]) | 0; v15 ^= v3; v15 = (v15 >>> 8) | (v15 << 24);
    v11 = (v11 + v15) | 0; v7 ^= v11; v7 = (v7 >>> 7) | (v7 << 25);
    v0 = (v0 + v5 + m[mo + s[8]]) | 0; v15 ^= v0; v15 = (v15 >>> 16) | (v15 << 16);
    v10 = (v10 + v15) | 0; v5 ^= v10; v5 = (v5 >>> 12) | (v5 << 20);
    v0 = (v0 + v5 + m[mo + s[9]]) | 0; v15 ^= v0; v15 = (v15 >>> 8) | (v15 << 24);
    v10 = (v10 + v15) | 0; v5 ^= v10; v5 = (v5 >>> 7) | (v5 << 25);
    v1 = (v1 + v6 + m[mo + s[10]]) | 0; v12 ^= v1; v12 = (v12 >>> 16) | (v12 << 16);
    v11 = (v11 + v12) | 0; v6 ^= v11; v6 = (v6 >>> 12) | (v6 << 20);
    v1 = (v1 + v6 + m[mo + s[11]]) | 0; v12 ^= v1; v12 = (v12 >>> 8) | (v12 << 24);
    v11 = (v11 + v12) | 0; v6 ^= v11; v6 = (v6 >>> 7) | (v6 << 25);
    v2 = (v2 + v7 + m[mo + s[12]]) | 0; v13 ^= v2; v13 = (v13 >>> 16) | (v13 << 16);
    v8 = (v8 + v13) | 0; v7 ^= v8; v7 = (v7 >>> 12) | (v7 << 20);
    v2 = (v2 + v7 + m[mo + s[13]]) | 0; v13 ^= v2; v13 = (v13 >>> 8) | (v13 << 24);
    v8 = (v8 + v13) | 0; v7 ^= v8; v7 = (v7 >>> 7) | (v7 << 25);
    v3 = (v3 + v4 + m[mo + s[14]]) | 0; v14 ^= v3; v14 = (v14 >>> 16) | (v14 << 16);
    v9 = (v9 + v14) | 0; v4 ^= v9; v4 = (v4 >>> 12) | (v4 << 20);
    v3 = (v3 + v4 + m[mo + s[15]]) | 0; v14 ^= v3; v14 = (v14 >>> 8) | (v14 << 24);
    v9 = (v9 + v14) | 0; v4 ^= v9; v4 = (v4 >>> 7) | (v4 << 25);
  }
  out[0] = v0 ^ v8; out[1] = v1 ^ v9; out[2] = v2 ^ v10; out[3] = v3 ^ v11;
  out[4] = v4 ^ v12; out[5] = v5 ^ v13; out[6] = v6 ^ v14; out[7] = v7 ^ v15;
  out[8] = v8 ^ cv[0]; out[9] = v9 ^ cv[1]; out[10] = v10 ^ cv[2]; out[11] = v11 ^ cv[3];
  out[12] = v12 ^ cv[4]; out[13] = v13 ^ cv[5]; out[14] = v14 ^ cv[6]; out[15] = v15 ^ cv[7];
}

/** 64 baytlık bloğu küçük sonlu sözcüklere çevirir (eksik baytlar sıfır). */
function words(bytes, off, len, m) {
  m.fill(0);
  for (let i = 0; i < len; i++) m[i >> 2] |= bytes[off + i] << (8 * (i & 3));
}

const tmp = new Uint32Array(16);

export class Blake3 {
  constructor() {
    this.cv = IV.slice();
    this.chunk = 0;           // bu öbeğin sayacı
    this.block = new Uint8Array(BLOCK_LEN);
    this.blockLen = 0;
    this.blocksDone = 0;      // öbekte sıkıştırılmış blok
    this.stack = [];          // birleşmeyi bekleyen alt ağaç zincir değerleri
    this.m = new Uint32Array(16);
  }

  chunkLen() {
    return this.blocksDone * BLOCK_LEN + this.blockLen;
  }

  startFlag() {
    return this.blocksDone === 0 ? CHUNK_START : 0;
  }

  /**
   * Öbeğin dolu bloğunu sıkıştırır (son blok değilse). `u32` verilirse
   * sözcükler kopyalanmadan doğrudan girdiden okunur.
   */
  flushBlock(src, off, u32) {
    if (u32) {
      compress(this.cv, u32, off >>> 2, this.chunk, BLOCK_LEN, this.startFlag(), tmp);
    } else {
      words(src, off, BLOCK_LEN, this.m);
      compress(this.cv, this.m, 0, this.chunk, BLOCK_LEN, this.startFlag(), tmp);
    }
    this.cv.set(tmp.subarray(0, 8));
    this.blocksDone++;
  }

  /** Öbek bitti: zincir değeri ağaca eklenir. */
  endChunk() {
    words(this.block, 0, this.blockLen, this.m);
    compress(this.cv, this.m, 0, this.chunk, this.blockLen, this.startFlag() | CHUNK_END, tmp);
    let cv = tmp.slice(0, 8);
    let total = this.chunk + 1;
    while (total % 2 === 0) {
      cv = parentCv(this.stack.pop(), cv);
      total = Math.floor(total / 2);
    }
    this.stack.push(cv);
    this.chunk++;
    this.cv = IV.slice();
    this.blockLen = 0;
    this.blocksDone = 0;
  }

  /** @param {Uint8Array} input */
  update(input) {
    let i = 0;
    const n = input.length;
    const u32 = LITTLE_ENDIAN && input.byteOffset % 4 === 0 ? new Uint32Array(input.buffer, input.byteOffset, n >>> 2) : null;
    while (i < n) {
      if (this.chunkLen() === CHUNK_LEN) this.endChunk();
      if (this.blockLen === BLOCK_LEN) {
        this.flushBlock(this.block, 0, null);
        this.blockLen = 0;
      }
      // Hızlı yol: tam bloklar arabelleğe kopyalanmadan sıkıştırılır; öbeğin ve
      // girdinin son bloğu sonuçlandırma için arabellekte bekler.
      while (this.blockLen === 0 && n - i > BLOCK_LEN && this.chunkLen() + BLOCK_LEN < CHUNK_LEN) {
        this.flushBlock(input, i, u32);
        i += BLOCK_LEN;
      }
      const take = Math.min(BLOCK_LEN - this.blockLen, CHUNK_LEN - this.chunkLen(), n - i);
      this.block.set(input.subarray(i, i + take), this.blockLen);
      this.blockLen += take;
      i += take;
    }
    return this;
  }

  /** @returns {Uint8Array} 32 bayt */
  digest() {
    words(this.block, 0, this.blockLen, this.m);
    let cv = this.cv;
    let m = this.m.slice();
    let counter = this.chunk;
    let len = this.blockLen;
    let flags = this.startFlag() | CHUNK_END;
    for (let k = this.stack.length - 1; k >= 0; k--) {
      compress(cv, m, 0, counter, len, flags, tmp);
      const right = tmp.slice(0, 8);
      m = new Uint32Array(16);
      m.set(this.stack[k], 0);
      m.set(right, 8);
      cv = IV;
      counter = 0;
      len = BLOCK_LEN;
      flags = PARENT;
    }
    compress(cv, m, 0, counter, len, flags | ROOT, tmp);
    const out = new Uint8Array(32);
    for (let w = 0; w < 8; w++) {
      out[4 * w] = tmp[w] & 0xff;
      out[4 * w + 1] = (tmp[w] >>> 8) & 0xff;
      out[4 * w + 2] = (tmp[w] >>> 16) & 0xff;
      out[4 * w + 3] = (tmp[w] >>> 24) & 0xff;
    }
    return out;
  }
}

function parentCv(left, right) {
  const m = new Uint32Array(16);
  m.set(left, 0);
  m.set(right, 8);
  const out = new Uint32Array(16);
  compress(IV, m, 0, 0, BLOCK_LEN, PARENT, out);
  return out.slice(0, 8);
}

export function toHex(bytes) {
  let s = "";
  for (const b of bytes) s += b.toString(16).padStart(2, "0");
  return s;
}

/** @param {Uint8Array} bytes */
export function blake3Hex(bytes) {
  return toHex(new Blake3().update(bytes).digest());
}
