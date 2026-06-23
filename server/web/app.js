// Clip PWA — iPhone/web client. Connects to the relay, shares the desktop's
// end-to-end encryption (PBKDF2-HMAC-SHA256 + AES-256-GCM via Web Crypto), shows
// synced clip history, copies on tap, and can send. The relay only sees ciphertext.

const PBKDF2_ITERATIONS = 200_000; // must match clip-sync::crypto::PBKDF2_ITERATIONS

const KIND_LABEL = { text: "Text", link: "Link", color: "Color", code: "Code",
  rich_text: "Rich text", image: "Image", files: "Files", video: "Video", audio: "Audio", binary: "Binary" };
const KIND_GLYPH = { text: "¶", link: "↗", color: "◉", code: "{}", rich_text: "✦",
  image: "▣", files: "🗂", video: "▶", audio: "♪", binary: "⬡" };

const $ = (id) => document.getElementById(id);
const enc = new TextEncoder();
const dec = new TextDecoder();
let ws = null, key = null;
const seen = new Map(); // id -> clip, newest-first render

// ---- crypto (identical scheme to the Rust desktop) ----
async function deriveKey(secret, account) {
  let salt = account;
  while (salt.length < 16) salt += "#"; // matches the Rust salt padding
  const base = await crypto.subtle.importKey("raw", enc.encode(secret), "PBKDF2", false, ["deriveKey"]);
  return crypto.subtle.deriveKey(
    { name: "PBKDF2", salt: enc.encode(salt), iterations: PBKDF2_ITERATIONS, hash: "SHA-256" },
    base, { name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
}
async function decryptFrame(data) {
  const iv = data.slice(0, 12), ct = data.slice(12);
  const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, ct);
  return JSON.parse(dec.decode(pt));
}
async function encryptClip(clip) {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, enc.encode(JSON.stringify(clip))));
  const out = new Uint8Array(12 + ct.length);
  out.set(iv, 0); out.set(ct, 12);
  return out;
}
async function sha256Hex(s) {
  const h = new Uint8Array(await crypto.subtle.digest("SHA-256", enc.encode(s)));
  return [...h].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// ---- helpers ----
const bytesToStr = (arr) => (arr ? dec.decode(new Uint8Array(arr)) : "");
function relTime(ms) {
  const s = Math.max(0, (Date.now() - ms) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return Math.floor(s / 60) + "m";
  if (s < 86400) return Math.floor(s / 3600) + "h";
  return Math.floor(s / 86400) + "d";
}
function toast(t) {
  const el = $("toast"); el.textContent = t; el.classList.add("show");
  setTimeout(() => el.classList.remove("show"), 1300);
}

// ---- rendering ----
function render() {
  const list = $("list");
  const clips = [...seen.values()].sort((a, b) => b.created_at - a.created_at);
  if (!clips.length) { list.innerHTML = '<div class="empty">No clips yet. Copy something on your desktop.</div>'; return; }
  list.innerHTML = "";
  for (const c of clips) {
    const row = document.createElement("div");
    row.className = "row";
    row.style.setProperty("--rail", `var(--k-${c.kind})`);
    const isImg = c.kind === "image" && c.body;
    const thumb = isImg
      ? `<img alt="" src="data:image/png;base64,${b64(c.body)}">`
      : (KIND_GLYPH[c.kind] || "•");
    row.innerHTML =
      `<div class="thumb">${thumb}</div>
       <div class="body">
         <div class="preview ${c.kind === "code" ? "mono" : ""}">${escapeHtml(c.preview)}</div>
         <div class="meta"><span class="chip">${KIND_LABEL[c.kind] || c.kind}</span><span class="time">${relTime(c.created_at)}</span></div>
       </div>`;
    row.onclick = () => copyClip(c);
    list.appendChild(row);
  }
}
function escapeHtml(s) { const d = document.createElement("div"); d.textContent = s; return d.innerHTML; }
function b64(arr) { let s = ""; const u = new Uint8Array(arr); for (let i = 0; i < u.length; i++) s += String.fromCharCode(u[i]); return btoa(s); }

async function copyClip(c) {
  const text = bytesToStr(c.body) || c.preview;
  // Secure context (HTTPS/localhost): full clipboard API, incl. images.
  if (navigator.clipboard && window.isSecureContext) {
    try {
      if (c.kind === "image" && c.body) {
        const blob = new Blob([new Uint8Array(c.body)], { type: "image/png" });
        await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      } else {
        await navigator.clipboard.writeText(text);
      }
      toast("Copied");
      return;
    } catch { /* fall through to legacy */ }
  }
  // Plain http (LAN): legacy textarea copy — works on a user tap. Text only.
  if (c.kind === "image") { toast("Image copy needs HTTPS — see setup"); return; }
  if (legacyCopy(text)) toast("Copied");
  else toast("Couldn't copy");
}

function legacyCopy(text) {
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.focus();
  ta.select();
  ta.setSelectionRange(0, text.length);
  let ok = false;
  try { ok = document.execCommand("copy"); } catch { ok = false; }
  document.body.removeChild(ta);
  return ok;
}

function ingest(clip) {
  seen.set(clip.id, clip);
  render();
}

// ---- connection ----
function setStatus(t, on) { const s = $("status"); s.textContent = t; s.classList.toggle("on", !!on); }

async function connect() {
  const account = $("account").value.trim();
  const secret = $("secret").value;
  if (!account || !secret) { toast("Enter account + passphrase"); return; }
  let relay = $("relay").value.trim();
  if (!relay) {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    relay = `${proto}://${location.host}/sync`;
  }
  localStorage.setItem("clip.account", account);
  localStorage.setItem("clip.secret", secret);
  localStorage.setItem("clip.relay", relay);

  key = await deriveKey(secret, account);
  let device = localStorage.getItem("clip.device");
  if (!device) { device = "phone-" + Math.random().toString(36).slice(2, 8); localStorage.setItem("clip.device", device); }

  $("connect").classList.add("hidden");
  $("list").classList.remove("hidden");
  $("composer").classList.remove("hidden");
  render();

  const url = `${relay}?account=${encodeURIComponent(account)}&device=${encodeURIComponent(device)}`;
  open(url);
}

function open(url) {
  setStatus("connecting…");
  ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  ws.onopen = () => setStatus("synced", true);
  ws.onclose = () => { setStatus("reconnecting…"); setTimeout(() => open(url), 2000); };
  ws.onerror = () => setStatus("error");
  ws.onmessage = async (e) => {
    try { ingest(await decryptFrame(new Uint8Array(e.data))); }
    catch { /* wrong key / not for us */ }
  };
}

async function send() {
  const text = $("msg").value;
  if (!text || !ws || ws.readyState !== 1) return;
  const body = [...enc.encode(text)];
  const clip = {
    id: "web-" + (await sha256Hex("text\0" + text)),
    kind: "text", preview: text.length > 400 ? text.slice(0, 400) + "…" : text,
    body, blob_ref: null, size: body.length, created_at: Date.now(), pinned: false,
  };
  ws.send(await encryptClip(clip));
  ingest(clip); // optimistic local echo
  $("msg").value = "";
}

// ---- boot ----
$("go").onclick = connect;
$("send").onclick = send;
$("msg").addEventListener("keydown", (e) => { if (e.key === "Enter") send(); });
$("account").value = localStorage.getItem("clip.account") || "";
$("secret").value = localStorage.getItem("clip.secret") || "";
$("relay").value = localStorage.getItem("clip.relay") || "";
if ($("account").value && $("secret").value) connect(); // auto-reconnect

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/sw.js").catch(() => {});
