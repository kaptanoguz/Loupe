// Loupe mobil aktarım sayfası: QR → oturum → ön kontrol → BLAKE3 → tus.
// Bağımlılık yok; sunucu (loupe-lan) bu dosyayı ikilinin içinden verir.

import { Blake3, toHex } from "./blake3.js";
import {
  CHUNK_BYTES,
  HASH_SLICE_BYTES,
  PRECHECK_BATCH,
  RateMeter,
  SESSION_STORAGE_KEY,
  TUS_VERSION,
  assetId,
  deviceNameFromUA,
  formatBytes,
  formatClock,
  formatRate,
  parseFragment,
  platformFromUA,
  retryDelay,
  splitByPrecheck,
  tusMetadata,
} from "./core.js";

const $ = (id) => document.getElementById(id);

const ui = {
  server: $("server"),
  pairing: $("view-pairing"),
  pairingText: $("pairing-text"),
  pairingHint: $("pairing-hint"),
  ready: $("view-ready"),
  files: $("files"),
  rate: $("s-rate"),
  eta: $("s-eta"),
  done: $("s-done"),
  skipped: $("s-skipped"),
  bar: $("bar"),
  bytes: $("s-bytes"),
  failed: $("s-failed"),
  current: $("current"),
  stop: $("stop"),
  resume: $("resume"),
  list: $("list"),
  conflict: $("conflict"),
  conflictName: $("conflict-name"),
  conflictAll: $("conflict-all"),
};

/** Oturum anahtarı: localStorage'da; ayrıca HttpOnly çerez olarak da gelir. */
let sessionKey = readStoredKey();

function readStoredKey() {
  try {
    return localStorage.getItem(SESSION_STORAGE_KEY);
  } catch {
    return null;
  }
}

function storeKey(key) {
  sessionKey = key;
  try {
    if (key) localStorage.setItem(SESSION_STORAGE_KEY, key);
    else localStorage.removeItem(SESSION_STORAGE_KEY);
  } catch {
    // Gizli sekme: çerez yine çalışır.
  }
}

function authHeaders(extra = {}) {
  const h = { ...extra };
  if (sessionKey) h.Authorization = `Bearer ${sessionKey}`;
  return h;
}

class SessionLost extends Error {}

// ───────────────────────────── eşleşme ─────────────────────────────

async function boot() {
  const { token } = parseFragment(location.hash);
  // Belirteç adres çubuğunda ve geçmişte kalmaz.
  if (location.hash) history.replaceState(null, "", location.pathname);
  if (token) {
    showPairing("Eşleşiyor…");
    const paired = await pair(token);
    if (paired) return showReady(paired);
  }
  const who = await whoami();
  if (who) return showReady(who);
  showPairing(
    token ? "Kodun süresi dolmuş ya da kullanılmış." : "Bu tarayıcı henüz eşleşmedi.",
    true,
  );
}

async function pair(token) {
  try {
    const res = await fetch("/m/session", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        token,
        device_name: deviceNameFromUA(navigator.userAgent),
        platform: platformFromUA(navigator.userAgent),
      }),
    });
    if (!res.ok) return null;
    const body = await res.json();
    storeKey(body.session_key);
    return body;
  } catch {
    return null;
  }
}

async function whoami() {
  try {
    const res = await fetch("/m/session", { headers: authHeaders() });
    if (res.ok) return await res.json();
    if (res.status === 401) storeKey(null);
  } catch {
    // Ağ yok: aşağıda eşleşme ekranı.
  }
  return null;
}

function showPairing(text, hint = false) {
  ui.pairing.hidden = false;
  ui.ready.hidden = true;
  ui.pairingText.textContent = text;
  ui.pairingHint.hidden = !hint;
}

function showReady(info) {
  ui.pairing.hidden = true;
  ui.ready.hidden = false;
  ui.server.textContent = `${info.server_name} · ${info.device_name}`;
}

// ───────────────────────────── kuyruk ─────────────────────────────

const state = {
  queue: [],
  running: false,
  stopped: false,
  xhr: null,
  totalBytes: 0,
  sentBytes: 0,
  done: 0,
  skipped: 0,
  failed: 0,
  meter: new RateMeter(),
  conflictChoice: null,
  wake: null,
};

ui.files.addEventListener("change", () => {
  const files = Array.from(ui.files.files || []);
  ui.files.value = "";
  if (files.length > 0) void enqueue(files);
});

ui.stop.addEventListener("click", () => {
  state.stopped = true;
  if (state.xhr) state.xhr.abort();
});

ui.resume.addEventListener("click", () => {
  ui.resume.hidden = true;
  void run();
});

window.addEventListener("beforeunload", (e) => {
  if (state.running) e.preventDefault();
});

async function enqueue(files) {
  const items = files.map((file) => ({ file, id: assetId(file), state: "waiting", row: row(file.name) }));
  ui.current.textContent = "Daha önce aktarılanlar denetleniyor…";
  let known = [];
  try {
    known = await precheck(items);
  } catch (e) {
    if (e instanceof SessionLost) return lost();
    // Ön kontrol yalnızca hızlandırır: olmazsa sunucu yine BLAKE3 ile ayırır.
  }
  const { skip } = splitByPrecheck(files, known);
  const skipped = new Set(skip);
  for (const it of items) {
    if (skipped.has(it.file)) {
      setRow(it, "skipped", "zaten var");
      state.skipped++;
    } else {
      setRow(it, "waiting", "sırada");
      state.queue.push(it);
      state.totalBytes += it.file.size;
    }
  }
  render();
  if (!state.running) void run();
}

async function precheck(items) {
  const known = [];
  for (let i = 0; i < items.length; i += PRECHECK_BATCH) {
    const batch = items.slice(i, i + PRECHECK_BATCH).map((it) => ({
      device_asset_id: it.id,
      modified_at: it.file.lastModified,
    }));
    const res = await fetch("/m/precheck", {
      method: "POST",
      headers: authHeaders({ "Content-Type": "application/json" }),
      body: JSON.stringify(batch),
    });
    if (res.status === 401) throw new SessionLost();
    if (!res.ok) throw new Error(`ön kontrol: ${res.status}`);
    known.push(...(await res.json()).known);
  }
  return known;
}

async function run() {
  state.running = true;
  state.stopped = false;
  ui.stop.hidden = false;
  await keepAwake(true);
  try {
    while (state.queue.length > 0 && !state.stopped) {
      const it = state.queue[0];
      try {
        await transfer(it);
        state.queue.shift();
      } catch (e) {
        if (e instanceof SessionLost) return lost();
        if (state.stopped) break;
        state.queue.shift();
        state.failed++;
        state.totalBytes -= it.file.size - (it.sent || 0);
        setRow(it, "failed", "hata");
        ui.current.textContent = `${it.file.name}: ${e.message}`;
      }
      render();
    }
  } finally {
    state.running = false;
    state.xhr = null;
    state.conflictChoice = null;
    ui.stop.hidden = true;
    ui.resume.hidden = !(state.stopped && state.queue.length > 0);
    await keepAwake(false);
    if (!state.stopped && state.queue.length === 0) ui.current.textContent = summary();
    render();
  }
}

function summary() {
  const parts = [`${state.done} aktarıldı`, `${state.skipped} atlandı`];
  if (state.failed > 0) parts.push(`${state.failed} hata`);
  return `Bitti: ${parts.join(" · ")}`;
}

async function transfer(it) {
  setRow(it, "active", "özet");
  const hash = await hashFile(it);
  if (state.stopped) throw new Error("durduruldu");
  let resolution;
  for (;;) {
    const created = await createUpload(it, hash, resolution);
    if (created.kind === "upload") {
      await upload(it, created.url, created.offset);
      return;
    }
    if (created.kind === "have") {
      countSkipped(it, created.reason);
      return;
    }
    // Çakışma: kullanıcıya sorulur ya da "tümüne uygula" kararı kullanılır.
    // "Atla" da sunucuya gider: bu sürüm "görüldü" olur, bir daha sorulmaz.
    resolution = state.conflictChoice ?? (await askConflict(it));
  }
}

function countSkipped(it, reason) {
  state.skipped++;
  state.totalBytes -= it.file.size;
  setRow(it, "skipped", reason);
}

async function hashFile(it) {
  const h = new Blake3();
  const f = it.file;
  for (let off = 0; off < f.size; off += HASH_SLICE_BYTES) {
    if (state.stopped) break;
    const buf = await f.slice(off, Math.min(f.size, off + HASH_SLICE_BYTES)).arrayBuffer();
    h.update(new Uint8Array(buf));
    ui.current.textContent = `${f.name} · özet ${Math.floor((100 * Math.min(f.size, off + HASH_SLICE_BYTES)) / Math.max(1, f.size))}%`;
  }
  return toHex(h.digest());
}

async function createUpload(it, hash, resolution) {
  for (let attempt = 0; ; attempt++) {
    let res;
    try {
      res = await fetch("/files", {
        method: "POST",
        headers: authHeaders({
          "Tus-Resumable": TUS_VERSION,
          "Upload-Length": String(it.file.size),
          "Upload-Metadata": tusMetadata({
            filename: it.file.name,
            device_asset_id: it.id,
            blake3: hash,
            modified_at: it.file.lastModified,
            resolution,
          }),
        }),
      });
    } catch (e) {
      await backoff(attempt, e);
      continue;
    }
    if (res.status === 401) throw new SessionLost();
    if (res.status === 201) {
      const loc = res.headers.get("Location");
      if (!loc) throw new Error("Location başlığı yok");
      return { kind: "upload", url: loc, offset: Number(res.headers.get("Upload-Offset") || 0) };
    }
    const status = res.headers.get("Loupe-Sync-Status");
    if (res.status === 200 && status === "already-synced") return { kind: "have", reason: "zaten var" };
    if (res.status === 200 && status === "skipped") return { kind: "have", reason: "atlandı" };
    if (res.status === 409 && status === "conflict") return { kind: "conflict" };
    if (res.status === 429 || res.status >= 500) {
      await backoff(attempt, new Error(`sunucu ${res.status}`));
      continue;
    }
    throw new Error(await errorText(res));
  }
}

async function errorText(res) {
  try {
    const b = await res.json();
    return b.message || b.error || `HTTP ${res.status}`;
  } catch {
    return `HTTP ${res.status}`;
  }
}

/** tus PATCH döngüsü; kopan bağlantıda HEAD ile kalınan yer sorulur. */
async function upload(it, url, startOffset) {
  const size = it.file.size;
  let offset = startOffset;
  // Önceki oturumdan gelen baytlar ilerlemeye sayılır, hıza sayılmaz.
  it.sent = offset;
  state.sentBytes += offset;
  setRow(it, "active", pct(offset, size));
  let attempt = 0;
  while (offset < size || size === 0) {
    if (state.stopped) throw new Error("durduruldu");
    const end = Math.min(size, offset + CHUNK_BYTES);
    try {
      const r = await patch(url, it.file.slice(offset, end), offset, (n, metered) => progress(it, n, metered));
      if (r.status === 401) throw new SessionLost();
      if (r.status === 204) {
        const next = Number(r.headers["upload-offset"]);
        progress(it, next - it.sent, false);
        offset = next;
        attempt = 0;
        setRow(it, "active", pct(offset, size));
        if (r.headers["loupe-sync-status"] === "verified" || offset >= size) {
          state.done++;
          setRow(it, "done", "tamam");
          return;
        }
        continue;
      }
      if (r.status === 409 || r.status === 460) {
        // Kayma ya da bozuk parça: sunucunun bildiği yerden devam.
        offset = await headOffset(url, it);
        continue;
      }
      if (r.status === 404 || r.status === 410) throw new Error("yükleme oturumu bulunamadı");
      if (r.status === 429 || r.status >= 500) throw new Error(`sunucu ${r.status}`);
      throw new Error(`HTTP ${r.status}`);
    } catch (e) {
      if (e instanceof SessionLost || state.stopped) throw e;
      if (/oturumu bulunamadı|^HTTP /.test(e.message)) throw e;
      await backoff(attempt++, e);
      offset = await headOffset(url, it);
    }
  }
}

/** XHR: fetch yükleme ilerlemesi vermez, XHR verir. */
function patch(url, blob, offset, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    state.xhr = xhr;
    xhr.open("PATCH", url);
    xhr.timeout = 120000;
    const headers = authHeaders({
      "Tus-Resumable": TUS_VERSION,
      "Upload-Offset": String(offset),
      "Content-Type": "application/offset+octet-stream",
    });
    for (const [k, v] of Object.entries(headers)) xhr.setRequestHeader(k, v);
    let last = 0;
    xhr.upload.onprogress = (e) => {
      onProgress(e.loaded - last, true);
      last = e.loaded;
    };
    xhr.onload = () => {
      state.xhr = null;
      resolve({
        status: xhr.status,
        headers: {
          "upload-offset": xhr.getResponseHeader("Upload-Offset"),
          "loupe-sync-status": xhr.getResponseHeader("Loupe-Sync-Status"),
        },
      });
    };
    const fail = (what) => () => {
      state.xhr = null;
      // Onaylanmamış baytlar geri alınır; doğrusu HEAD ile gelir.
      onProgress(-last, false);
      reject(new Error(what));
    };
    xhr.onerror = fail("bağlantı koptu");
    xhr.ontimeout = fail("zaman aşımı");
    xhr.onabort = fail("durduruldu");
    xhr.send(blob);
  });
}

async function headOffset(url, it) {
  for (let attempt = 0; ; attempt++) {
    try {
      const res = await fetch(url, { method: "HEAD", headers: authHeaders({ "Tus-Resumable": TUS_VERSION }) });
      if (res.status === 401) throw new SessionLost();
      if (res.status === 404 || res.status === 410) throw new Error("yükleme oturumu bulunamadı");
      if (res.ok) {
        const off = Number(res.headers.get("Upload-Offset"));
        progress(it, off - it.sent, false);
        return off;
      }
      throw new Error(`HEAD ${res.status}`);
    } catch (e) {
      if (e instanceof SessionLost || /oturumu bulunamadı/.test(e.message) || state.stopped) throw e;
      await backoff(attempt, e);
    }
  }
}

async function backoff(attempt, cause) {
  const wait = retryDelay(attempt);
  if (wait === null) throw cause;
  ui.current.textContent = `${cause.message} · ${Math.round(wait / 1000)} sn sonra yeniden denenecek`;
  await new Promise((r) => setTimeout(r, wait));
  if (state.stopped) throw new Error("durduruldu");
}

/** `metered`: yalnızca bu oturumda ağdan gerçekten geçen baytlar hıza sayılır. */
function progress(it, delta, metered) {
  if (!delta) return;
  it.sent = (it.sent || 0) + delta;
  state.sentBytes += delta;
  if (metered && delta > 0) state.meter.add(delta, performance.now());
  render();
}

function askConflict(it) {
  return new Promise((resolve) => {
    setRow(it, "conflict", "soruluyor");
    ui.conflictName.textContent = it.file.name;
    ui.conflictAll.checked = false;
    const done = () => {
      ui.conflict.removeEventListener("close", done);
      const choice = ui.conflict.returnValue || "skip";
      if (ui.conflictAll.checked) state.conflictChoice = choice;
      setRow(it, "active", "");
      resolve(choice);
    };
    ui.conflict.returnValue = "";
    ui.conflict.addEventListener("close", done);
    ui.conflict.showModal();
  });
}

function lost() {
  storeKey(null);
  state.queue = [];
  if (state.xhr) state.xhr.abort();
  showPairing("Bu tarayıcının erişimi kaldırıldı ya da süresi doldu.", true);
}

async function keepAwake(on) {
  try {
    if (on && !state.wake && navigator.wakeLock) state.wake = await navigator.wakeLock.request("screen");
    if (!on && state.wake) {
      await state.wake.release();
      state.wake = null;
    }
  } catch {
    // Ekran kilidi API'si yoksa ipucu metni yeterli.
  }
}

// ───────────────────────────── görünüm ─────────────────────────────

function row(name) {
  const li = document.createElement("li");
  const n = document.createElement("span");
  n.className = "name";
  n.textContent = name;
  const s = document.createElement("span");
  s.className = "state";
  li.append(n, s);
  ui.list.prepend(li);
  return li;
}

function setRow(it, st, text) {
  it.state = st;
  it.row.dataset.state = st;
  it.row.lastChild.textContent = text;
  if (st === "active") ui.current.textContent = `${it.file.name}${text ? ` · ${text}` : ""}`;
}

function pct(a, b) {
  return `${b > 0 ? Math.floor((100 * a) / b) : 100}%`;
}

let frame = 0;
function render() {
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    const now = performance.now();
    const rate = state.running ? state.meter.rate(now) : 0;
    const remaining = Math.max(0, state.totalBytes - state.sentBytes);
    ui.rate.textContent = formatRate(rate);
    ui.eta.textContent = rate > 0 && remaining > 0 ? formatClock(remaining / rate) : "—";
    ui.done.textContent = String(state.done);
    ui.skipped.textContent = String(state.skipped);
    const p = state.totalBytes > 0 ? Math.min(100, (100 * state.sentBytes) / state.totalBytes) : 0;
    ui.bar.style.width = `${p}%`;
    ui.bar.parentElement.setAttribute("aria-valuenow", String(Math.round(p)));
    ui.bytes.textContent =
      state.totalBytes > 0 ? `${formatBytes(state.sentBytes)} / ${formatBytes(state.totalBytes)}` : "—";
    ui.failed.hidden = state.failed === 0;
    ui.failed.textContent = `${state.failed} hata`;
  });
}

void boot();
