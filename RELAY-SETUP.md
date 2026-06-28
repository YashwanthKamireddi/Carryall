# Put your relay online — free, permanent, no credit card

This gives your phone one stable HTTPS address to reach Carryall from **anywhere**,
using a **Cloudflare Tunnel** and **your own domain**. It's free, needs **no card**,
needs **no port-forwarding**, and installs **nothing on the phone**.

Because Carryall is end-to-end encrypted, the relay only ever sees ciphertext — so
exposing it through a public URL is safe. The relay itself listens on **localhost
only**; the tunnel is its single entrypoint.

> **Where it runs:** the relay + tunnel run on a machine you own (your desktop is
> ideal — it's already running the capture daemon). Your phone can sync whenever
> that machine is on. No card-required cloud host needed.

---

## Prerequisites
- A domain you own (any registrar — name.com, Namecheap, etc.). We'll use a
  subdomain, e.g. `clip.yourdomain.com`.
- `carryall-relay` built or installed. From source: `cargo build --release -p carryall-relay`
  → binary at `target/release/carryall-relay`. Copy it onto your PATH:
  ```bash
  mkdir -p ~/.local/bin && cp target/release/carryall-relay ~/.local/bin/
  ```

---

## Step 1 — Add your EXISTING domain to Cloudflare (free, no card)
> ⚠️ Use **"Add / Connect a domain you already own"** — NOT the **Registrations** or
> **Transfers** pages (those are for *buying* a domain from Cloudflare). Your
> registration stays where it is; Cloudflare only takes over DNS.

1. Sign up / log in at <https://dash.cloudflare.com> (Free plan — never asks for a card).
2. **Account home → "+ Add" → "Connect a domain"** (or "Add a domain"). Enter
   `yourdomain.com` → **Continue**. Pick the **Free** plan.
3. Cloudflare **scans and imports your current DNS records**. **Verify your portfolio's
   records are in the list** (the A/CNAME for `yourdomain.com` and `www`, and anything
   your host — Vercel/Netlify/GitHub Pages/name.com — told you to add). If they're
   there, your portfolio will keep working. **Continue.**
4. Cloudflare shows **two nameservers** (e.g. `xxx.ns.cloudflare.com`). **Copy both.**
5. At your registrar, switch to those nameservers:
   - **name.com:** Domain List → your domain → **Manage Nameservers** → choose
     **Custom** / "Use your own" → paste the two Cloudflare nameservers → Save.
     (The "Automatic Renewal — Action Required" notice is unrelated; ignore it.)
   - **Namecheap:** Domain List → Manage → **Nameservers → Custom DNS** → paste → save.
6. Wait for Cloudflare to email **"your domain is active"** (minutes, sometimes a few
   hours). Your portfolio resolves through Cloudflare to the same host — unchanged and
   reversible. You now manage DNS in Cloudflare.

## Step 2 — Install cloudflared
```bash
# macOS
brew install cloudflared
# Debian/Ubuntu
sudo apt install cloudflared      # or: see https://pkg.cloudflare.com
# Arch
sudo pacman -S cloudflared
# Windows (PowerShell, winget)
winget install --id Cloudflare.cloudflared
```

## Step 3 — Create the tunnel + DNS route (one time)
```bash
cloudflared tunnel login                       # opens a browser; authorize yourdomain.com
cloudflared tunnel create carryall             # prints a TUNNEL_ID + writes ~/.cloudflared/<ID>.json
cloudflared tunnel route dns carryall clip.yourdomain.com   # creates the DNS record for you
```

## Step 4 — Configure the tunnel
Copy the template and fill in your `TUNNEL_ID` (from Step 3) and hostname:
```bash
mkdir -p ~/.cloudflared
cp contrib/cloudflared-config.yml ~/.cloudflared/config.yml
# edit ~/.cloudflared/config.yml: set tunnel:, credentials-file:, and the hostname
```
It should look like:
```yaml
tunnel: <your-TUNNEL_ID>
credentials-file: ~/.cloudflared/<your-TUNNEL_ID>.json
ingress:
  - hostname: clip.yourdomain.com
    service: http://localhost:8787
  - service: http_status:404
```

## Step 5 — Run the relay + tunnel
```bash
# Relay on localhost only (the tunnel is the public door):
CLIP_RELAY_ADDR=127.0.0.1:8787 carryall-relay &
# Tunnel:
cloudflared tunnel run carryall
```
Test it from any network:
```bash
curl https://clip.yourdomain.com/health      # → ok
```

## Step 6 — Make it permanent (autostart)
**Relay** (systemd user service):
```bash
cp contrib/carryall-relay.service ~/.config/systemd/user/
systemctl --user enable --now carryall-relay
```
**Tunnel** (cloudflared ships its own installer):
```bash
sudo cloudflared service install      # runs the tunnel on boot
```
Now both come up automatically — set it and forget it.

## Step 7 — Point Carryall at your relay
Edit `~/.config/clip/config.toml` on each desktop and set:
```toml
relay = "wss://clip.yourdomain.com/sync"
```
Restart the daemon (`systemctl --user restart carryall-daemon`), then re-pair your
phone so the QR encodes the new URL:
```bash
carryall pair
```
Scan it — your phone now reaches Carryall from anywhere. Done.

---

## Verify
- `curl https://clip.yourdomain.com/health` → `ok` (from off your network, e.g. phone data).
- Copy something on the desktop → it appears on the phone within a moment.

## Troubleshooting
- **`curl` fails / DNS not found** — Cloudflare nameservers not active yet (Step 1), or
  the `route dns` record is missing. Re-run Step 3's last command.
- **Connects but won't sync** — confirm the relay is up: `curl http://localhost:8787/health`,
  and that `relay` in `config.toml` is `wss://…/sync` (note `wss`, and the `/sync` path).
- **Phone says "can't decrypt"** — account/passphrase mismatch, unrelated to the tunnel;
  re-pair with `carryall pair`.
- **Tunnel "is already running"** — you have both `cloudflared tunnel run` and the installed
  service active; keep only one.

## Alternative (no domain move)
Prefer not to move DNS to Cloudflare? **Tailscale Funnel** also exposes the relay over
HTTPS for free with no card, but on a `*.ts.net` address instead of your domain:
`tailscale funnel 8787`. Use that if you'd rather leave Namecheap DNS untouched.
