// Clip PWA — world-class, native-feeling web client.
// E2E (PBKDF2-HMAC-SHA256 + AES-256-GCM, matching the Rust desktop), local
// IndexedDB persistence (history survives reload/offline), search, pinning,
// sections, copy-on-tap with feedback. The relay only ever sees ciphertext.

const PBKDF2_ITERATIONS = 200_000; // must match clip-sync::crypto

const $ = (id) => document.getElementById(id);
const enc = new TextEncoder();
const dec = new TextDecoder();
let ws = null, key = null, account = "", device = "";
const clips = new Map(); // id -> clip (+ local `pinned`)
let query = "";

// ---------- crypto (identical scheme to the desktop) ----------
async function deriveKey(secret, acct) {
  let salt = acct;
  while (salt.length < 16) salt += "#";
  const base = await crypto.subtle.importKey("raw", enc.encode(secret), "PBKDF2", false, ["deriveKey"]);
  return crypto.subtle.deriveKey(
    { name: "PBKDF2", salt: enc.encode(salt), iterations: PBKDF2_ITERATIONS, hash: "SHA-256" },
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
function openDb(acct) {
  return new Promise((res) => {
    const req = indexedDB.open("clip-" + acct, 1);
    req.onupgradeneeded = () => req.result.createObjectStore("clips", { keyPath: "id" });
    req.onsuccess = () => res(req.result);
    req.onerror = () => res(null);
  });
}
function dbPut(clip) { try { db?.transaction("clips", "readwrite").objectStore("clips").put(clip); } catch {} }
function dbDel(id) { try { db?.transaction("clips", "readwrite").objectStore("clips").delete(id); } catch {} }
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
const PIN_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9.5 4h5l-1 6 3 2.5H7.5L10.5 10z"/><line x1="12" y1="15" x2="12" y2="20.5"/></svg>';

// ---------- helpers ----------
const bytesToStr = (arr) => (arr ? dec.decode(new Uint8Array(arr)) : "");
const b64 = (arr) => { let s = ""; const u = new Uint8Array(arr); for (let i = 0; i < u.length; i++) s += String.fromCharCode(u[i]); return btoa(s); };
const escapeHtml = (s) => { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; };
function relTime(ms) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "now"; if (s < 3600) return Math.floor(s / 60) + "m";
  if (s < 86400) return Math.floor(s / 3600) + "h"; return Math.floor(s / 86400) + "d";
}
function toast(t) { const el = $("toast"); el.textContent = t; el.classList.add("show"); clearTimeout(el._t); el._t = setTimeout(() => el.classList.remove("show"), 1400); }
function setStatus(text, cls) { $("status").className = cls || ""; $("status-text").textContent = text; }

// ---------- render ----------
function render() {
  const list = $("list");
  const q = query.trim().toLowerCase();
  let arr = [...clips.values()];
  if (q) arr = arr.filter((c) => (c.preview || "").toLowerCase().includes(q));
  arr.sort((a, b) => b.created_at - a.created_at);
  const pinned = arr.filter((c) => c.pinned);
  const recent = arr.filter((c) => !c.pinned);

  if (!arr.length) {
    list.innerHTML = `<div class="empty">${q ? "No matches." : "No clips yet.<br>Copy something on your desktop."}</div>`;
    return;
  }
  const section = (label, items) => items.length
    ? `<div class="sec">${label}</div>` + items.map(rowHtml).join("") : "";
  list.innerHTML = (q ? arr.map(rowHtml).join("") : section("Pinned", pinned) + section("Recent", recent));
}
function rowHtml(c) {
  const isImg = c.kind === "image" && c.body;
  const tile = isImg ? `<img alt="" src="data:image/png;base64,${b64(c.body)}">`
    : c.kind === "color" ? `<div class="swatch" style="background:${escapeHtml(c.preview)}"></div>`
    : svgIcon(c.kind);
  return `<div class="row" data-id="${c.id}">
      <div class="tile">${tile}</div>
      <div class="meatball" data-act="copy" data-id="${c.id}">
        <div class="ptext ${c.kind === "code" ? "mono" : ""}">${escapeHtml(c.preview)}</div>
        <div class="pmeta">${KIND_LABEL[c.kind] || c.kind} · ${relTime(c.created_at)}</div>
      </div>
      <button class="pin ${c.pinned ? "on" : ""}" data-act="pin" data-id="${c.id}">${PIN_SVG}</button>
    </div>`;
}

function ingest(clip, persist = true) {
  const prev = clips.get(clip.id);
  if (prev?.pinned) clip.pinned = true; // keep local pin
  clips.set(clip.id, clip);
  if (persist) dbPut(clip);
  render();
}

// ---------- actions ----------
async function copyClip(c) {
  const text = bytesToStr(c.body) || c.preview;
  if (navigator.clipboard && window.isSecureContext) {
    try {
      if (c.kind === "image" && c.body) {
        await navigator.clipboard.write([new ClipboardItem({ "image/png": new Blob([new Uint8Array(c.body)], { type: "image/png" }) })]);
      } else await navigator.clipboard.writeText(text);
      toast("Copied"); return;
    } catch {}
  }
  if (c.kind === "image") { toast("Image copy needs HTTPS"); return; }
  const ta = document.createElement("textarea"); ta.value = text; ta.style.position = "fixed"; ta.style.opacity = "0";
  document.body.appendChild(ta); ta.select();
  toast(document.execCommand("copy") ? "Copied" : "Couldn't copy");
  document.body.removeChild(ta);
}
function togglePin(c) { c.pinned = !c.pinned; clips.set(c.id, c); dbPut(c); render(); toast(c.pinned ? "Pinned" : "Unpinned"); }

async function send() {
  const text = $("msg").value;
  if (!text || ws?.readyState !== 1) return;
  const body = [...enc.encode(text)];
  const clip = { id: "web-" + (await sha256Hex("text\0" + text)), kind: "text",
    preview: text.length > 400 ? text.slice(0, 400) + "…" : text, body, blob_ref: null,
    size: body.length, created_at: Date.now(), pinned: false };
  ws.send(await encryptClip(clip));
  ingest(clip);
  $("msg").value = "";
}

// ---------- connection ----------
function openWs(url) {
  setStatus("connecting…", "connecting");
  ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  ws.onopen = () => setStatus("synced", "on");
  ws.onclose = () => { setStatus("reconnecting…", "connecting"); setTimeout(() => openWs(url), 2000); };
  ws.onerror = () => setStatus("error", "");
  ws.onmessage = async (e) => { try { ingest(await decryptFrame(e.data)); } catch {} };
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

  key = await deriveKey(secret, account);
  db = await openDb(account);
  (await dbAll()).forEach((c) => clips.set(c.id, c)); // load saved history (offline-ready)

  $("connect").classList.add("hidden");
  $("searchbar").classList.remove("hidden");
  $("list").classList.remove("hidden");
  $("composer").classList.remove("hidden");
  render();
  openWs(`${relay}?account=${encodeURIComponent(account)}&device=${encodeURIComponent(device)}`);
}

// ---------- wire up ----------
$("go").onclick = connect;
$("send").onclick = send;
$("msg").addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
$("search").addEventListener("input", (e) => { query = e.target.value; render(); });
$("list").addEventListener("click", (e) => {
  const el = e.target.closest("[data-act]"); if (!el) return;
  const c = clips.get(el.dataset.id); if (!c) return;
  if (el.dataset.act === "copy") copyClip(c);
  else if (el.dataset.act === "pin") togglePin(c);
});
$("account").value = localStorage.getItem("clip.account") || "";
$("secret").value = localStorage.getItem("clip.secret") || "";
$("relay").value = localStorage.getItem("clip.relay") || "";
if ($("account").value && $("secret").value) connect();
if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});
