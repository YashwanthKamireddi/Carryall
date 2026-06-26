// Clip PWA — world-class, native-feeling web client.
// E2E (PBKDF2-HMAC-SHA256 + AES-256-GCM, matching the Rust desktop), local
// IndexedDB persistence (history survives reload/offline), search, pinning,
// sections, copy-on-tap with feedback. The relay only ever sees ciphertext.

const PBKDF2_ITERATIONS = 200_000; // must match clip-sync::crypto

const $ = (id) => document.getElementById(id);
const enc = new TextEncoder();
const dec = new TextDecoder();
let ws = null, key = null, account = "", device = "";
// Wrong-passphrase detection: every incoming frame fails to decrypt, so surface
// it instead of looking "synced" forever with nothing appearing.
let decryptOk = 0, decryptFail = 0, pwWarned = false;
const clips = new Map(); // id -> clip (+ local `pinned`)
let query = "";
let filter = "all"; // type filter: all | text | link | image | files

// Map a clip kind to a filter bucket (matches the segmented control).
function filterBucket(kind) {
  if (kind === "link") return "link";
  if (kind === "image") return "image";
  if (kind === "files" || kind === "video" || kind === "audio") return "files";
  return "text"; // text, code, color, rich_text, binary
}

// ---------- crypto (identical scheme to the desktop) ----------
async function deriveKey(secret, acct) {
  // Salt = the account's UTF-8 BYTES padded with '#' (0x23) to >=16 bytes — pad
  // bytes, not JS chars, so a non-ASCII account matches the Rust daemon's key.
  const salt = [...enc.encode(acct)];
  while (salt.length < 16) salt.push(0x23);
  const base = await crypto.subtle.importKey("raw", enc.encode(secret), "PBKDF2", false, ["deriveKey"]);
  return crypto.subtle.deriveKey(
    { name: "PBKDF2", salt: new Uint8Array(salt), iterations: PBKDF2_ITERATIONS, hash: "SHA-256" },
    base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
}
async function decryptFrame(data) {
  const u = new Uint8Array(data);
  const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: u.slice(0, 12) }, key, u.slice(12));
  return JSON.parse(dec.decode(pt));
}
async function encryptClip(clip) {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, enc.encode(JSON.stringify(clip))));
  const out = new Uint8Array(12 + ct.length); out.set(iv, 0); out.set(ct, 12); return out;
}
async function sha256Hex(s) {
  const h = new Uint8Array(await crypto.subtle.digest("SHA-256", enc.encode(s)));
  return [...h].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// ---------- IndexedDB (per-account local history) ----------
let db = null;
const tombstones = new Map(); // id -> deletedAt (ms); blocks backlog resurrection
function openDb(acct) {
  return new Promise((res) => {
    const req = indexedDB.open("clip-" + acct, 3);
    req.onupgradeneeded = () => {
      const d = req.result;
      if (!d.objectStoreNames.contains("clips")) d.createObjectStore("clips", { keyPath: "id" });
      if (!d.objectStoreNames.contains("tombstones")) d.createObjectStore("tombstones");
      // Full file/image bytes, keyed by blob_ref — so received files survive a
      // reload/offline instead of depending on a relay backlog re-send.
      if (!d.objectStoreNames.contains("blobs")) d.createObjectStore("blobs");
    };
    req.onsuccess = () => res(req.result);
    req.onerror = () => res(null);
  });
}
function dbPut(clip) { try { db?.transaction("clips", "readwrite").objectStore("clips").put(clip); } catch {} }
function dbDel(id) { try { db?.transaction("clips", "readwrite").objectStore("clips").delete(id); } catch {} }
// Blob bytes are stored on disk (IndexedDB), loaded into RAM only on demand
// (when a clip is opened) so a big history doesn't sit in memory.
function dbPutBlob(ref, bytes) { try { db?.transaction("blobs", "readwrite").objectStore("blobs").put(bytes, ref); } catch {} }
function dbDelBlob(ref) { try { if (ref) db?.transaction("blobs", "readwrite").objectStore("blobs").delete(ref); } catch {} }
function dbGetBlob(ref) {
  return new Promise((res) => {
    if (!db || !ref) return res(null);
    try {
      const r = db.transaction("blobs", "readonly").objectStore("blobs").get(ref);
      r.onsuccess = () => res(r.result ? new Uint8Array(r.result) : null);
      r.onerror = () => res(null);
    } catch { res(null); }
  });
}
function dbBlobRefs() {
  return new Promise((res) => {
    if (!db) return res([]);
    try {
      const r = db.transaction("blobs", "readonly").objectStore("blobs").getAllKeys();
      r.onsuccess = () => res(r.result || []);
      r.onerror = () => res([]);
    } catch { res([]); }
  });
}
function dbTombstone(id, at) { try { db?.transaction("tombstones", "readwrite").objectStore("tombstones").put(at, id); } catch {} }
function dbUntombstone(id) { try { db?.transaction("tombstones", "readwrite").objectStore("tombstones").delete(id); } catch {} }
function dbTombstones() {
  return new Promise((res) => {
    if (!db) return res([]);
    const store = db.transaction("tombstones", "readonly").objectStore("tombstones");
    const kr = store.getAllKeys(), vr = store.getAll();
    let keys = null, vals = null;
    const done = () => { if (keys && vals) res(keys.map((k, i) => [k, vals[i]])); };
    kr.onsuccess = () => { keys = kr.result || []; done(); };
    vr.onsuccess = () => { vals = vr.result || []; done(); };
    kr.onerror = vr.onerror = () => res([]);
  });
}
function dbAll() {
  return new Promise((res) => {
    if (!db) return res([]);
    const r = db.transaction("clips", "readonly").objectStore("clips").getAll();
    r.onsuccess = () => res(r.result || []);
    r.onerror = () => res([]);
  });
}

// ---------- icons (shared vocabulary with desktop) ----------
const ICON = {
  text: '<path d="M7 8.5C5.3 9.3 4.5 10.7 4.5 12.7H7.8V16H4.5V12.7"/><path d="M15 8.5c-1.7.8-2.5 2.2-2.5 4.2H15.8V16H12.5V12.7"/>',
  link: '<path d="M9.5 14.5l5-5"/><path d="M11 6.8l1.2-1.2a3.8 3.8 0 0 1 5.4 5.4l-1.2 1.2"/><path d="M13 17.2l-1.2 1.2a3.8 3.8 0 0 1-5.4-5.4l1.2-1.2"/>',
  code: '<path d="M9 8l-4 4 4 4"/><path d="M15 8l4 4-4 4"/>',
  rich_text: '<rect x="5" y="4" width="14" height="16" rx="2"/><line x1="8.5" y1="9" x2="15.5" y2="9"/><line x1="8.5" y1="13" x2="13" y2="13"/>',
  image: '<rect x="3.5" y="5" width="17" height="14" rx="2.5"/><circle cx="8.5" cy="10" r="1.4"/><path d="M20.5 15.5l-4.5-4.5L6 19"/>',
  files: '<path d="M4 7.5a2 2 0 0 1 2-2h3.2l2 2H18a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z"/>',
  video: '<rect x="3.5" y="6" width="17" height="12" rx="2.5"/><path d="M10.5 9.5l4.5 2.5-4.5 2.5z" fill="currentColor" stroke="none"/>',
  audio: '<path d="M9 17V6l10-2v11"/><circle cx="6.5" cy="17" r="2.4"/><circle cx="16.5" cy="15" r="2.4"/>',
  binary: '<path d="M12 3l8 4.5v9L12 21l-8-4.5v-9z"/>',
};
const KIND_LABEL = { text: "Text", link: "Link", color: "Color", code: "Code", rich_text: "Rich text",
  image: "Image", files: "Files", video: "Video", audio: "Audio", binary: "Binary" };
const svgIcon = (kind) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICON[kind] || ICON.binary}</svg>`;
// Refined SF-Symbol-style action glyphs.
const PIN_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 17v4"/><path d="M9 3h6l-.8 3.2a3 3 0 0 0 .9 2.9l1.4 1.3a1 1 0 0 1 .3.8v.3a.5.5 0 0 1-.5.5h-9a.5.5 0 0 1-.5-.5v-.3a1 1 0 0 1 .3-.8l1.4-1.3a3 3 0 0 0 .9-2.9z"/></svg>';
const PIN_FILL_SVG = '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M9 3h6l-.85 3.4a3 3 0 0 0 .9 2.9l1.45 1.35a1 1 0 0 1 .35.76.6.6 0 0 1-.6.59H12.8V21a.8.8 0 0 1-1.6 0v-8H7.75a.6.6 0 0 1-.6-.59 1 1 0 0 1 .35-.76L8.95 9.3a3 3 0 0 0 .9-2.9z"/></svg>';
const TRASH_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M5 7h14"/><path d="M9 7V5.5A1.5 1.5 0 0 1 10.5 4h3A1.5 1.5 0 0 1 15 5.5V7"/><path d="M7 7l.8 11.2A1.8 1.8 0 0 0 9.6 20h4.8a1.8 1.8 0 0 0 1.8-1.8L17 7"/></svg>';
const DOWNLOAD_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v10"/><path d="M8 11l4 4 4-4"/><path d="M5 19h14"/></svg>';

// ---------- helpers ----------
const bytesToStr = (arr) => (arr ? dec.decode(new Uint8Array(arr)) : "");

// Cached, lowercased searchable text per clip: the preview PLUS the full body for
// text-like kinds, so search matches content beyond the 400-char preview. Keyed
// by the content-addressed id (never changes for the same content), computed once.
const searchCache = new Map();
function searchText(c) {
  let s = searchCache.get(c.id);
  if (s === undefined) {
    s = c.preview || "";
    const k = normKind(c.kind);
    if ((k === "text" || k === "code" || k === "link" || k === "rich_text") && c.body) {
      const full = bytesToStr(c.body);
      if (full.length > s.length) s = full; // body holds the complete, untruncated text
    }
    s = s.toLowerCase();
    searchCache.set(c.id, s);
  }
  return s;
}
const b64 = (arr) => { let s = ""; const u = new Uint8Array(arr); for (let i = 0; i < u.length; i++) s += String.fromCharCode(u[i]); return btoa(s); };
const escapeHtml = (s) => { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; };
function relTime(ms) {
  if (typeof ms !== "number" || !isFinite(ms)) return ""; // guard NaN/undefined
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "now"; if (s < 3600) return Math.floor(s / 60) + "m";
  if (s < 86400) return Math.floor(s / 3600) + "h"; return Math.floor(s / 86400) + "d";
}
/** Normalize a possibly-malformed kind to a known one. */
const normKind = (k) => (KIND_LABEL[k] ? k : "binary");
function toast(t) { const el = $("toast"); el.textContent = t; el.classList.add("show"); clearTimeout(el._t); el._t = setTimeout(() => el.classList.remove("show"), 1400); }
// A whisper of haptic feedback on tap (Android/most Chromium; iOS Safari ignores
// it harmlessly — the spring animations carry the feel there).
function haptic(p) { try { navigator.vibrate && navigator.vibrate(p); } catch {} }
function setStatus(text, cls) { $("status").className = cls || ""; $("status-text").textContent = text; }

// ---------- render ----------
let flashId = null; // id of a just-arrived clip to pulse once (consumed by render)
let popId = null; // id of a clip whose pin should spring-"pop" once (consumed by render)
function render() {
  const list = $("list");
  const q = query.trim().toLowerCase();
  let arr = [...clips.values()];
  if (filter !== "all") arr = arr.filter((c) => filterBucket(normKind(c.kind)) === filter);
  if (q) arr = arr.filter((c) => searchText(c).includes(q));
  arr.sort((a, b) => b.created_at - a.created_at);
  const pinned = arr.filter((c) => c.pinned);
  const recent = arr.filter((c) => !c.pinned);

  if (!arr.length) {
    const msg = q ? "No matches." : filter !== "all" ? "Nothing here yet." : "No clips yet.<br>Copy something on your desktop.";
    list.innerHTML = `<div class="empty">${msg}</div>`;
    flashId = popId = null;
    return;
  }
  const group = (items) => `<div class="group">${items.map(cellHtml).join("")}</div>`;
  const section = (label, items) => (items.length ? `<div class="sec">${label}</div>` + group(items) : "");
  list.innerHTML = q ? group(arr) : section("Pinned", pinned) + section("Recent", recent);
  flashId = popId = null; // one-shot: consumed by this render, never replays
}
function faviconUrl(u) {
  // Tolerate scheme-less links (e.g. "www.x.com") so they still get a favicon.
  try { return `https://www.google.com/s2/favicons?domain=${new URL(/^https?:\/\//i.test(u) ? u : "https://" + u).hostname}&sz=64`; }
  catch { return null; }
}
function cellHtml(c) {
  const kind = normKind(c.kind);
  const isImg = kind === "image" && c.body;
  const fav = kind === "link" ? faviconUrl(c.preview) : null;
  const inner = isImg ? `<img alt="" src="data:image/png;base64,${b64(c.body)}">`
    : kind === "color" ? `<div class="swatch" style="background:${escapeHtml(c.preview)}"></div>`
    : fav ? `${svgIcon("link")}<img class="favicon" src="${fav}" onerror="this.style.display='none'">`
    : svgIcon(kind);
  // File rows show a download affordance + readiness in the meta line. The age
  // lives in its own span so it can tick live without re-rendering the list.
  const label = KIND_LABEL[kind] ? KIND_LABEL[kind] + " · " : "";
  let meta = `${label}<span class="age" data-t="${c.created_at}">${relTime(c.created_at)}</span>`;
  if (FILE_KINDS.has(kind) && !hasBlob(c.blob_ref) && c.blob_ref) {
    meta += " · receiving…"; // the download button itself signals "ready"
  }
  // Only offer the download affordance once the blob has actually arrived.
  const actionGlyph = FILE_KINDS.has(kind) && hasBlob(c.blob_ref) ? DOWNLOAD_SVG : null;
  const flash = c.id === flashId ? " flash" : "";
  return `<div class="cell${flash}" data-act="open" data-id="${c.id}">
      <div class="leadicon" style="background:var(--c-${kind})">${inner}</div>
      <div class="body">
        <div class="ptext ${kind === "code" ? "mono" : ""}">${escapeHtml(c.preview || "")}</div>
        <div class="pmeta">${meta}</div>
      </div>
      <div class="actions">
        ${actionGlyph ? `<button class="act dl" data-act="open" data-id="${c.id}" aria-label="Download">${actionGlyph}</button>` : ""}
        <button class="act pin ${c.pinned ? "on" : ""}${c.id === popId ? " pop" : ""}" data-act="pin" data-id="${c.id}" aria-label="Pin">${c.pinned ? PIN_FILL_SVG : PIN_SVG}</button>
        <button class="act del" data-act="del" data-id="${c.id}" aria-label="Delete">${TRASH_SVG}</button>
      </div>
    </div>`;
}

function ingest(clip, persist = true) {
  // Reject malformed clips (no id or no usable content) so no junk rows appear.
  if (!clip || !clip.id || (!clip.preview && !clip.body)) return;
  if (typeof clip.created_at !== "number" || !isFinite(clip.created_at)) clip.created_at = Date.now();
  // Deleted on a device: a stale backlog replay (older created_at) stays dead;
  // a genuine re-copy afterwards (newer created_at) clears the tombstone.
  if (tombstones.has(clip.id)) {
    if (clip.created_at <= tombstones.get(clip.id)) return;
    tombstones.delete(clip.id); dbUntombstone(clip.id);
  }
  clip.kind = normKind(clip.kind);
  const prev = clips.get(clip.id);
  if (prev?.pinned) clip.pinned = true; // keep local pin
  if (!prev) flashId = clip.id; // pulse genuinely-new arrivals once
  clips.set(clip.id, clip);
  if (persist) dbPut(clip);
  render();
}

// ---------- received blobs (file/image content reassembled from chunks) ----------
const FILE_KINDS = new Set(["files", "video", "audio"]);
const pendingBlobs = new Map(); // ref -> { parts:[], have, n }
const blobs = new Map(); // ref -> Uint8Array (complete, in RAM)
const persistedBlobRefs = new Set(); // refs whose bytes are saved in IndexedDB (may not be in RAM)

// True if we have the full blob anywhere (RAM or on-disk) — used for the
// download/copy affordance without forcing the bytes into memory.
function hasBlob(ref) { return !!ref && (blobs.has(ref) || persistedBlobRefs.has(ref)); }
// Fetch the full blob bytes, loading from IndexedDB into RAM on first use.
async function getBlob(ref) {
  if (!ref) return null;
  if (blobs.has(ref)) return blobs.get(ref);
  const bytes = await dbGetBlob(ref);
  if (bytes) blobs.set(ref, bytes); // cache for repeat opens
  return bytes;
}
// Record a completed blob: RAM + persisted to disk so it survives reload.
function saveBlob(ref, bytes) { blobs.set(ref, bytes); dbPutBlob(ref, bytes); persistedBlobRefs.add(ref); }

function onChunk(frame) {
  const { blob_chunk: ref, i, n, b64 } = frame;
  if (!ref || typeof n !== "number") return;
  if (hasBlob(ref)) return; // already have the whole blob — ignore backlog re-sends
  let e = pendingBlobs.get(ref);
  if (!e) { e = { parts: new Array(n).fill(null), have: 0 }; pendingBlobs.set(ref, e); }
  if (i < e.parts.length && e.parts[i] === null) {
    const bin = atob(b64); const u = new Uint8Array(bin.length);
    for (let k = 0; k < bin.length; k++) u[k] = bin.charCodeAt(k);
    e.parts[i] = u; e.have++;
  }
  if (e.have === e.parts.length) {
    let len = 0; for (const p of e.parts) len += p.length;
    const whole = new Uint8Array(len); let off = 0;
    for (const p of e.parts) { whole.set(p, off); off += p.length; }
    saveBlob(ref, whole); pendingBlobs.delete(ref);
    render(); // a file row may now be ready to download
  }
}

// Decode the CLA1 archive → [{name, data}].
function decodeArchive(u8) {
  const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  let o = 0;
  if (u8.length < 8 || String.fromCharCode(u8[0], u8[1], u8[2], u8[3]) !== "CLA1") return null;
  o = 4; const count = dv.getUint32(o, true); o += 4;
  const out = [];
  for (let k = 0; k < count; k++) {
    const nlen = dv.getUint16(o, true); o += 2;
    const name = dec.decode(u8.subarray(o, o + nlen)); o += nlen;
    const dlen = Number(dv.getBigUint64(o, true)); o += 8;
    out.push({ name, data: u8.subarray(o, o + dlen) }); o += dlen;
  }
  return out;
}

function downloadBlob(name, bytes, mime) {
  // Folder entries arrive with a relative path (e.g. "proj/src/main.rs"); a
  // browser download is a flat file, so use just the basename for a clean name.
  const base = String(name || "file").split(/[/\\]/).pop() || "file";
  const url = URL.createObjectURL(new Blob([bytes], { type: mime || "application/octet-stream" }));
  const a = document.createElement("a");
  a.href = url; a.download = base; document.body.appendChild(a); a.click();
  a.remove(); setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// ---------- actions ----------
async function copyText(text) {
  if (navigator.clipboard && window.isSecureContext) {
    try { await navigator.clipboard.writeText(text); return true; } catch {}
  }
  const ta = document.createElement("textarea"); ta.value = text; ta.style.position = "fixed"; ta.style.opacity = "0";
  document.body.appendChild(ta); ta.select();
  const ok = (() => { try { return document.execCommand("copy"); } catch { return false; } })();
  document.body.removeChild(ta); return ok;
}

// Returns true if the clip was COPIED to the clipboard (so the caller can give
// a "copied" acknowledgment); false for downloads / not-yet-ready.
async function openClip(c) {
  // Files/video/audio → download the REAL file(s), not the path text.
  if (FILE_KINDS.has(c.kind)) {
    const whole = await getBlob(c.blob_ref);
    if (!whole) { toast("Receiving file…"); return false; }
    const entries = decodeArchive(whole);
    if (entries && entries.length) {
      entries.forEach((f) => downloadBlob(f.name, f.data));
      toast(entries.length > 1 ? `Downloading ${entries.length} files` : "Downloading");
    } else {
      downloadBlob(c.preview || "file", whole); toast("Downloading");
    }
    return false;
  }
  // Image → copy/download the FULL-resolution blob (c.body is only the small
  // preview thumbnail; the real image is the streamed blob). Fall back to the
  // thumbnail only if the full blob hasn't finished arriving yet.
  if (c.kind === "image" && (hasBlob(c.blob_ref) || c.body)) {
    const full = await getBlob(c.blob_ref);
    if (!full && c.blob_ref) { toast("Receiving image…"); return false; }
    const bytes = full || new Uint8Array(c.body);
    if (navigator.clipboard && window.isSecureContext && window.ClipboardItem) {
      try {
        await navigator.clipboard.write([new ClipboardItem({ "image/png": new Blob([bytes], { type: "image/png" }) })]);
        toast("Copied"); return true;
      } catch {}
    }
    // Clean filename: drop the " · 1920×1080" suffix and any existing image
    // extension so we don't end up with e.g. "photo.png.png".
    const base = (c.preview || "image").replace(/\s*·.*$/, "").replace(/\.(png|jpe?g|gif|webp|bmp)$/i, "").trim() || "image";
    downloadBlob(base + ".png", bytes, "image/png");
    toast("Downloading"); return false;
  }
  // Text-like → copy.
  const ok = await copyText(bytesToStr(c.body) || c.preview);
  toast(ok ? "Copied" : "Couldn't copy");
  return ok;
}

async function togglePin(c) {
  c.pinned = !c.pinned; clips.set(c.id, c); dbPut(c);
  if (c.pinned) popId = c.id; // spring-pop the pin on (one-shot, consumed by render)
  haptic(c.pinned ? 14 : 8);
  render(); toast(c.pinned ? "Pinned" : "Unpinned");
  if (ws?.readyState === 1 && key) { try { ws.send(await encryptClip({ pin: c.id, pinned: c.pinned })); } catch {} }
}
function applyPin(id, pinned) {
  const c = clips.get(id); if (!c || !!c.pinned === !!pinned) return;
  c.pinned = !!pinned; clips.set(id, c); dbPut(c); render();
}
function forgetBlob(ref) {
  if (!ref) return;
  blobs.delete(ref); persistedBlobRefs.delete(ref); dbDelBlob(ref);
}
async function deleteClip(c) {
  const at = Date.now();
  clips.delete(c.id); dbDel(c.id); forgetBlob(c.blob_ref);
  tombstones.set(c.id, at); dbTombstone(c.id, at); render(); toast("Deleted");
  // Propagate the delete to your other devices (desktop daemon + other phones).
  if (ws?.readyState === 1 && key) { try { ws.send(await encryptClip({ delete: c.id })); } catch {} }
}
function applyDelete(id) {
  tombstones.set(id, Date.now()); dbTombstone(id, Date.now());
  if (!clips.has(id)) return;
  const c = clips.get(id); clips.delete(id); dbDel(id);
  forgetBlob(c?.blob_ref); render();
}

// Clear all non-pinned clips (pinned items are kept), syncing each removal to
// your other devices via the same tombstone+delete path. Two-tap confirm.
let clearArmed = false, clearTimer = null;
function disarmClear() {
  clearArmed = false; clearTimeout(clearTimer);
  const b = $("clearall"); b.textContent = "Clear"; b.classList.remove("armed");
}
function onClearTap() {
  const b = $("clearall");
  if (!clearArmed) {
    if (![...clips.values()].some((c) => !c.pinned)) { toast("Nothing to clear"); return; }
    clearArmed = true; b.textContent = "Clear all?"; b.classList.add("armed"); haptic(10);
    clearTimer = setTimeout(disarmClear, 3000);
    return;
  }
  disarmClear();
  clearAll();
}
async function clearAll() {
  const toClear = [...clips.values()].filter((c) => !c.pinned);
  if (!toClear.length) return;
  const at = Date.now();
  for (const c of toClear) {
    clips.delete(c.id); dbDel(c.id); forgetBlob(c.blob_ref);
    tombstones.set(c.id, at); dbTombstone(c.id, at);
  }
  render(); haptic([5, 24, 5]); toast(`Cleared ${toClear.length}`);
  if (ws?.readyState === 1 && key) {
    for (const c of toClear) { try { ws.send(await encryptClip({ delete: c.id })); } catch {} }
  }
}

// A lone URL (no surrounding whitespace) should sync as a link, matching how the
// desktop classifier tags copied links — so it shows a favicon, not plain text.
function isUrl(s) {
  const t = s.trim();
  if (/\s/.test(t)) return false;
  return /^https?:\/\/[^\s]+\.[^\s]+$/i.test(t) || /^www\.[^\s]+\.[^\s]+$/i.test(t);
}

// ---------- preview / edit sheet (long-press a clip) ----------
let sheetClip = null;
function closeSheet() {
  $("sheet").classList.remove("show");
  $("sheetbg").classList.remove("show");
  sheetClip = null;
}
async function openSheet(c) {
  sheetClip = c; haptic(12);
  const kind = normKind(c.kind);
  const content = $("sheetcontent"), actions = $("sheetactions");
  const editable = ["text", "code", "link", "rich_text", "binary"].includes(kind);
  if (kind === "image") {
    content.innerHTML = `<img class="preview" alt="">`;
    const bytes = (await getBlob(c.blob_ref)) || (c.body ? new Uint8Array(c.body) : null);
    if (bytes) content.querySelector("img").src = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
  } else if (FILE_KINDS.has(kind)) {
    const whole = await getBlob(c.blob_ref);
    const entries = whole && decodeArchive(whole);
    const names = entries && entries.length ? entries.map((e) => e.name) : [c.preview || "file"];
    content.innerHTML = `<div class="files">${names.map((n) => `<div class="f">${escapeHtml(n)}</div>`).join("")}</div>`;
  } else {
    content.innerHTML = `<textarea class="${kind === "code" ? "mono" : ""}" spellcheck="false"></textarea>`;
    content.querySelector("textarea").value = bytesToStr(c.body) || c.preview || "";
  }
  const btns = [];
  if (FILE_KINDS.has(kind)) {
    btns.push(`<button class="sbtn primary" data-s="download">Download</button>`);
  } else {
    btns.push(`<button class="sbtn primary" data-s="copy">Copy</button>`);
    if (editable) btns.push(`<button class="sbtn" data-s="send">Send edit</button>`);
    if (kind === "image") btns.push(`<button class="sbtn" data-s="download">Download</button>`);
  }
  btns.push(`<button class="sbtn" data-s="pin">${c.pinned ? "Unpin" : "Pin"}</button>`);
  btns.push(`<button class="sbtn danger" data-s="delete">Delete</button>`);
  actions.innerHTML = btns.join("");
  $("sheetbg").classList.add("show");
  $("sheet").classList.add("show");
}

// Send a text/link clip (shared by the composer and the preview-sheet "Send edit").
async function sendText(text) {
  if (!text || ws?.readyState !== 1) return false;
  haptic(10);
  const kind = isUrl(text) ? "link" : "text";
  const body = [...enc.encode(text)];
  const clip = { id: "web-" + (await sha256Hex(kind + "\0" + text)), kind,
    preview: text.length > 400 ? text.slice(0, 400) + "…" : text, body, blob_ref: null,
    size: body.length, created_at: Date.now(), pinned: false };
  ws.send(await encryptClip(clip));
  ingest(clip);
  return true;
}
async function send() {
  if (await sendText($("msg").value)) $("msg").value = "";
}

const CHUNK = 256 * 1024; // larger chunks = far fewer encrypt/encode round-trips (faster uploads)

// Build the CLA1 archive (same format the daemon packs/unpacks):
// "CLA1" | u32 count | [u16 nameLen | name | u64 dataLen | data]*
function buildArchive(items) {
  let total = 4 + 4;
  for (const it of items) total += 2 + new TextEncoder().encode(it.name).byteLength + 8 + it.data.byteLength;
  const buf = new Uint8Array(total);
  const dv = new DataView(buf.buffer);
  let o = 0;
  buf.set([67, 76, 65, 49], o); o += 4;            // "CLA1"
  dv.setUint32(o, items.length, true); o += 4;
  for (const it of items) {
    const nb = new TextEncoder().encode(it.name);
    dv.setUint16(o, nb.byteLength, true); o += 2;
    buf.set(nb, o); o += nb.byteLength;
    dv.setBigUint64(o, BigInt(it.data.byteLength), true); o += 8;
    buf.set(it.data, o); o += it.data.byteLength;
  }
  return buf;
}

// Fast base64 of a byte chunk (process in blocks to avoid huge call stacks).
function b64encode(u8) {
  let s = "";
  const STEP = 0x8000;
  for (let i = 0; i < u8.length; i += STEP) s += String.fromCharCode.apply(null, u8.subarray(i, i + STEP));
  return btoa(s);
}

// Stream a blob (already-built bytes) to peers as chunk frames after its clip.
async function streamBlob(ref, bytes) {
  const n = Math.max(1, Math.ceil(bytes.byteLength / CHUNK));
  for (let i = 0; i < n; i++) {
    const part = bytes.subarray(i * CHUNK, (i + 1) * CHUNK);
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const frame = { blob_chunk: ref, i, n, b64: b64encode(part) };
    const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, enc.encode(JSON.stringify(frame))));
    const out = new Uint8Array(12 + ct.length); out.set(iv, 0); out.set(ct, 12);
    ws.send(out);
  }
}

// Downscaled PNG thumbnail (+ real dimensions) for an image file.
async function imageThumb(file) {
  const bmp = await createImageBitmap(file);
  const scale = Math.min(1, 400 / Math.max(bmp.width, bmp.height));
  const w = Math.max(1, Math.round(bmp.width * scale)), h = Math.max(1, Math.round(bmp.height * scale));
  const c = new OffscreenCanvas(w, h);
  c.getContext("2d").drawImage(bmp, 0, 0, w, h);
  const blob = await c.convertToBlob({ type: "image/png" });
  return { bytes: new Uint8Array(await blob.arrayBuffer()), w: bmp.width, h: bmp.height };
}

// Send one image as a real IMAGE clip (thumbnail inline, full image streamed),
// so it shows as an image and pastes as an image on the desktop.
async function sendImage(file) {
  const full = new Uint8Array(await file.arrayBuffer());
  let body = [], preview = file.name || "Image";
  try { const t = await imageThumb(file); body = [...t.bytes]; preview = `Image · ${t.w}×${t.h}`; } catch {}
  const ref = "img-" + (await sha256Hex(b64encode(full)));
  const clip = { id: ref, kind: "image", preview, body, blob_ref: ref, size: full.byteLength, created_at: Date.now(), pinned: false };
  saveBlob(ref, full); // so our own list can act on it too (and survive reload)
  ws.send(await encryptClip(clip));
  await streamBlob(ref, full);
  ingest(clip);
}

// Send non-image files bundled as one archive (real files on the other side).
async function sendFiles(files) {
  const items = await Promise.all(files.map(async (f) => ({ name: f.name, data: new Uint8Array(await f.arrayBuffer()) })));
  const archive = buildArchive(items);
  const ref = "f-" + (await sha256Hex(b64encode(archive)));
  // Show the actual names, not a vague "2 files", so the row is identifiable.
  const names = files.map((f) => f.name);
  let preview = names.join(", ");
  if (preview.length > 48) preview = `${names[0]} +${names.length - 1} more`;
  const clip = { id: ref, kind: "files", preview, body: [...enc.encode(files.map((f) => f.name).join("\n"))],
    blob_ref: ref, size: archive.byteLength, created_at: Date.now(), pinned: false };
  saveBlob(ref, archive);
  ws.send(await encryptClip(clip));
  await streamBlob(ref, archive);
  ingest(clip);
}

// Size caps mirror the desktop daemon (cli/src/blobs.rs): full-quality transfer,
// but bounded so a huge pick can't blow up the tab's memory silently.
const MAX_FILE_BYTES = 256 * 1024 * 1024; // per file
const MAX_TOTAL_BYTES = 1024 * 1024 * 1024; // per multi-file selection
const fmtMB = (b) => (b / 1048576).toFixed(b < 10485760 ? 1 : 0) + " MB";

// Dispatch a selection: images send as images, the rest bundle as files.
async function sendAttachments(fileList) {
  let all = [...fileList];
  if (!all.length || ws?.readyState !== 1) return;
  // Reject individually-too-big files up front with a clear message.
  const toobig = all.filter((f) => f.size > MAX_FILE_BYTES);
  all = all.filter((f) => f.size <= MAX_FILE_BYTES);
  if (toobig.length) toast(`${toobig.length === 1 ? `"${toobig[0].name}"` : `${toobig.length} files`} too large (max ${fmtMB(MAX_FILE_BYTES)})`);
  if (!all.length) return;
  if (all.reduce((s, f) => s + f.size, 0) > MAX_TOTAL_BYTES) { toast(`Selection too large (max ${fmtMB(MAX_TOTAL_BYTES)})`); return; }
  const images = all.filter((f) => f.type.startsWith("image/"));
  const others = all.filter((f) => !f.type.startsWith("image/"));
  toast(`Sending ${all.length} item${all.length > 1 ? "s" : ""}…`);
  for (const img of images) await sendImage(img);
  if (others.length) await sendFiles(others);
  toast("Sent");
}

// ---------- connection ----------
function openWs(url) {
  setStatus("connecting…", "connecting");
  ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  ws.onopen = () => setStatus("synced", "on");
  ws.onclose = () => { setStatus("reconnecting…", "connecting"); setTimeout(() => openWs(url), 2000); };
  ws.onerror = () => setStatus("error", "");
  ws.onmessage = async (e) => {
    let pt;
    try {
      const u = new Uint8Array(e.data);
      pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv: u.slice(0, 12) }, key, u.slice(12));
    } catch {
      // Decrypt failed — almost always a passphrase/account mismatch. Warn once
      // after a few consecutive failures with nothing ever decrypting.
      if (++decryptFail >= 3 && decryptOk === 0 && !pwWarned) {
        pwWarned = true;
        setStatus("can't decrypt", "");
        toast("Can't decrypt — check your account & passphrase match your desktop");
      }
      return;
    }
    decryptOk++;
    try {
      const obj = JSON.parse(dec.decode(pt));
      if (obj && obj.blob_chunk) onChunk(obj); // file/image content
      else if (obj && obj.delete) applyDelete(obj.delete); // remote delete
      else if (obj && obj.pin) applyPin(obj.pin, obj.pinned); // remote pin toggle
      else ingest(obj); // clip metadata
    } catch {} // a single malformed frame shouldn't kill the stream
  };
}

async function connect() {
  account = $("account").value.trim();
  const secret = $("secret").value;
  if (!account || !secret) { toast("Enter account + passphrase"); return; }
  if (!window.isSecureContext || !crypto.subtle) {
    toast("Open over HTTPS — http on a LAN IP can't encrypt");
    return;
  }
  let relay = $("relay").value.trim();
  if (!relay) relay = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/sync`;
  localStorage.setItem("clip.account", account);
  localStorage.setItem("clip.secret", secret);
  localStorage.setItem("clip.relay", relay);
  device = localStorage.getItem("clip.device") || ("phone-" + Math.random().toString(36).slice(2, 8));
  localStorage.setItem("clip.device", device);

  decryptOk = decryptFail = 0; pwWarned = false; // reset passphrase-health counters
  key = await deriveKey(secret, account);
  db = await openDb(account);
  (await dbTombstones()).forEach(([id, at]) => tombstones.set(id, at)); // deleted ids stay dead
  (await dbBlobRefs()).forEach((ref) => persistedBlobRefs.add(ref)); // know which files we still have (bytes loaded on demand)
  (await dbAll()).forEach((c) => { if (!tombstones.has(c.id)) clips.set(c.id, c); }); // load saved history (offline-ready)

  $("connect").classList.add("hidden");
  $("nav").classList.remove("hidden");
  $("searchbar").classList.remove("hidden");
  $("clearall").classList.remove("hidden");
  $("filters").classList.remove("hidden");
  $("list").classList.add("entering"); // one-shot smooth entrance
  setTimeout(() => $("list").classList.remove("entering"), 600);
  $("list").classList.remove("hidden");
  $("composer").classList.remove("hidden");
  render();
  openWs(`${relay}?account=${encodeURIComponent(account)}&device=${encodeURIComponent(device)}`);
}

// ---------- wire up ----------
$("go").onclick = connect;
$("send").onclick = send;
$("clearall").onclick = onClearTap;
$("msg").addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
$("filters").addEventListener("click", (e) => {
  const btn = e.target.closest("button[data-filter]");
  if (!btn || btn.dataset.filter === filter) return;
  haptic(6);
  filter = btn.dataset.filter;
  for (const b of $("filters").querySelectorAll("button")) b.classList.toggle("on", b === btn);
  render();
});
$("attach").onclick = () => { haptic(8); $("fileinput").click(); };
$("fileinput").addEventListener("change", (e) => {
  if (e.target.files?.length) sendAttachments(e.target.files);
  e.target.value = ""; // allow re-selecting the same file
});

// Paste a file/image straight into the composer (like ChatGPT/Gemini) — no need
// to open the file picker. Pasted plain text still types into the box.
$("msg").addEventListener("paste", (e) => {
  const files = e.clipboardData?.files;
  if (files && files.length) { e.preventDefault(); sendAttachments(files); }
});
// Drag-and-drop files anywhere on the window to send them.
document.addEventListener("dragover", (e) => { e.preventDefault(); document.body.classList.add("dragging"); });
document.addEventListener("dragleave", (e) => { if (e.relatedTarget === null) document.body.classList.remove("dragging"); });
document.addEventListener("drop", (e) => {
  e.preventDefault(); document.body.classList.remove("dragging");
  if (e.dataTransfer?.files?.length && ws?.readyState === 1) sendAttachments(e.dataTransfer.files);
});
$("search").addEventListener("input", (e) => {
  query = e.target.value;
  $("searchclear").classList.toggle("hidden", !query);
  render();
});
$("searchclear").onclick = () => {
  query = ""; $("search").value = ""; $("searchclear").classList.add("hidden");
  render(); $("search").focus();
};
$("list").addEventListener("click", (e) => {
  const el = e.target.closest("[data-act]"); if (!el) return;
  if (Date.now() - lpSuppress < 600) return; // a long-press just opened the sheet
  const c = clips.get(el.dataset.id); if (!c) return;
  if (el.dataset.act === "open") {
    haptic(8);
    const cell = el.closest(".cell");
    openClip(c).then((copied) => {
      if (copied && cell) { cell.classList.add("copied"); setTimeout(() => cell.classList.remove("copied"), 700); }
    });
  }
  else if (el.dataset.act === "pin") togglePin(c);
  else if (el.dataset.act === "del") {
    // Swipe-and-fade the row out before it leaves the list (feels native).
    haptic([5, 24, 5]);
    const cell = el.closest(".cell");
    if (cell) { cell.classList.add("removing"); setTimeout(() => deleteClip(c), 190); }
    else deleteClip(c);
  }
});
// Long-press a clip → open the preview/edit sheet (short tap still copies).
let lpTimer = null, lpStart = null, lpSuppress = 0;
$("list").addEventListener("pointerdown", (e) => {
  const cell = e.target.closest(".cell");
  if (!cell || e.target.closest("button")) return; // row buttons handle themselves
  const c = clips.get(cell.dataset.id); if (!c) return;
  lpStart = { x: e.clientX, y: e.clientY };
  clearTimeout(lpTimer);
  lpTimer = setTimeout(() => { lpStart = null; lpSuppress = Date.now(); openSheet(c); }, 450);
});
$("list").addEventListener("pointermove", (e) => {
  if (lpStart && (Math.abs(e.clientX - lpStart.x) > 10 || Math.abs(e.clientY - lpStart.y) > 10)) {
    clearTimeout(lpTimer); lpStart = null; // moved → it's a scroll, not a long-press
  }
});
const lpCancel = () => { clearTimeout(lpTimer); lpStart = null; };
$("list").addEventListener("pointerup", lpCancel);
$("list").addEventListener("pointercancel", lpCancel);
$("list").addEventListener("pointerleave", lpCancel);

// Sheet actions.
$("sheetactions").addEventListener("click", async (e) => {
  const b = e.target.closest("[data-s]"); if (!b || !sheetClip) return;
  const c = sheetClip, ta = $("sheetcontent").querySelector("textarea");
  switch (b.dataset.s) {
    case "copy":
      if (ta) toast((await copyText(ta.value)) ? "Copied" : "Couldn't copy");
      else await openClip(c); // image → copy full-res
      break;
    case "send":
      if (ta && ta.value.trim()) { await sendText(ta.value); toast("Sent"); }
      break;
    case "download": {
      if (normKind(c.kind) === "image") {
        const bytes = (await getBlob(c.blob_ref)) || (c.body ? new Uint8Array(c.body) : null);
        if (bytes) { downloadBlob((c.preview || "image").replace(/\s*·.*$/, "").replace(/\.(png|jpe?g|gif|webp|bmp)$/i, "") + ".png", bytes, "image/png"); toast("Downloading"); }
      } else {
        const whole = await getBlob(c.blob_ref);
        if (!whole) { toast("Receiving file…"); break; }
        const en = decodeArchive(whole);
        if (en && en.length) { en.forEach((f) => downloadBlob(f.name, f.data)); toast(en.length > 1 ? `Downloading ${en.length} files` : "Downloading"); }
        else { downloadBlob(c.preview || "file", whole); toast("Downloading"); }
      }
      break;
    }
    case "pin": togglePin(c); b.textContent = c.pinned ? "Unpin" : "Pin"; break;
    case "delete": deleteClip(c); closeSheet(); break;
  }
});
$("sheetbg").addEventListener("click", closeSheet);
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeSheet(); });

$("account").value = localStorage.getItem("clip.account") || "";
$("secret").value = localStorage.getItem("clip.secret") || "";
$("relay").value = localStorage.getItem("clip.relay") || "";
if ($("account").value && $("secret").value) connect();
if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});

// Keep relative timestamps fresh ("now" → "2m" → "1h") without re-rendering the
// list — update only the age spans in place, so nothing flickers or loses scroll.
setInterval(() => {
  for (const el of document.querySelectorAll(".age[data-t]")) {
    const s = relTime(+el.dataset.t);
    if (el.textContent !== s) el.textContent = s;
  }
}, 30000);
