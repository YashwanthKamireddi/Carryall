# Deploying the Carryall relay (permanent, anywhere, 24/7)

The relay is a tiny stateless service that fans out **end-to-end-encrypted**
clip frames between an account's devices and keeps a bounded per-account backlog
so offline devices catch up on reconnect. It never sees plaintext, so hosting it
anywhere is safe.

## Option A — Fly.io (recommended: free-ish, global, auto-TLS)

From the repo root:

```bash
# one-time
curl -L https://fly.io/install.sh | sh
fly auth login

# deploy (config is in server/fly.toml; Dockerfile in server/Dockerfile)
fly launch --copy-config --config server/fly.toml --dockerfile server/Dockerfile
fly deploy --config server/fly.toml
```

You get a permanent URL like `https://carryall-relay.fly.dev`. TLS is automatic, which
also satisfies the iPhone PWA's secure-context needs (install + image copy).

## Option B — any Docker host (VPS, Railway, Render, …)

```bash
docker build -f server/Dockerfile -t carryall-relay .
docker run -p 8787:8787 carryall-relay
```

Put it behind a TLS-terminating reverse proxy (Caddy/nginx) with a domain.

## Point the apps at the deployed relay

- **iPhone / web (PWA):** open `https://<your-relay>/` in Safari. The relay URL is
  inferred from the page, so no extra config — just enter your account + passphrase.
- **Desktop:** run with sync enabled:
  ```bash
  CLIP_ACCOUNT=<you> CLIP_SECRET=<passphrase> \
  CLIP_RELAY_URL=wss://<your-relay>/sync \
  carryall
  ```
  (`wss://` for a TLS relay; the desktop client supports it.)

## Scale & durability

**Vertical:** one small instance handles many thousands of WebSocket
connections — broadcast rooms are cheap. Most deployments need nothing more.

**Horizontal (sticky sessions — recommended):** the relay routes purely by the
`account` query param, so put a load balancer in front that **consistent-hashes
on `account`**. Every device of an account lands on the same instance, so its
room and backlog are local — no cross-instance bus needed, and it scales to as
many instances as you like.

- Fly.io: hashing on the path/query is configured at the proxy; or run one app
  per region and let Anycast route.
- nginx: `hash $arg_account consistent;` in an `upstream` block.
- The backlog is **durable by default** (a SQLite file in the working dir; the
  image uses `/data`). Mount a **persistent volume** at `/data` so it survives
  restarts/redeploys. Override the path with `CLIP_RELAY_DB=<path>`, or set
  `CLIP_RELAY_DB=memory` to opt out of persistence.

**Multi-instance shared bus (for HA where any instance serves any account):**
set `CLIP_REDIS_URL` (e.g. `redis://host:6379`) on every instance. Each relay
publishes encrypted frames to Redis and subscribes to all rooms, so a device can
connect to *any* instance and still receive clips published on another — no
sticky routing needed. Redis carries ciphertext only. Verified across two
instances + Valkey.

**Limits:** max frame size 8 MiB (large blobs are chunked client-side);
per-account backlog bounded (default 500 frames) with a global cap (100k frames)
so an open relay can't be filled; per-account connection cap 32.

**Privacy:** the server stores/forwards ciphertext only — losing it leaks nothing.
