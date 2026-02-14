# ACP Architecture Review — GLM-5 + Kit Synthesis

_Feb 14, 2026 — GLM-5 ran the critique, Kit compiled and expanded_

## Critical Issues (in order of severity)

### 1. 🔴 No NAT Traversal — The Design Breaks Off-LAN

mDNS is strictly link-local. The moment agents are on different networks (Hans traveling, office vs home, Tailscale overlay), discovery fails completely. This is the most fundamental architectural issue because:

- It limits ACP to a toy for two machines on the same desk
- OpenClaw upstream won't accept a LAN-only protocol
- Even our own use case (Hans in London, agents in Phoenix) requires NAT traversal

**Fix:** Pluggable discovery from day one. mDNS as one backend, but also:
- Static peer config (IP:port in config file — works over Tailscale/VPN immediately)
- Optional rendezvous server for true NAT traversal (STUN/TURN-like)

### 2. 🔴 No Forward Secrecy

Static X25519 keys → long-lived symmetric key. If EITHER agent's private key is compromised:
- **All past messages** can be decrypted (no PFS)
- **All future messages** can be decrypted (until keys are manually rotated)
- No key rotation mechanism exists

**Fix:** Ephemeral key exchange per session (not per message — too expensive). On each TCP reconnect, generate ephemeral X25519 keypair, do a fresh DH. Static keys authenticate (via signing), ephemeral keys encrypt. This is essentially what TLS 1.3 does.

**Or:** Just use QUIC (see #6).

### 3. 🟡 QUIC Would Solve Multiple Problems Simultaneously

We're hand-rolling:
- Encryption (ChaCha20-Poly1305) → QUIC includes this
- Framing (length-prefixed TCP) → QUIC has streams
- Reliability → QUIC handles this
- Connection migration → QUIC supports IP changes (mobile agents)
- NAT traversal → QUIC's connection IDs survive NAT rebinding

The `quinn` crate is mature Rust QUIC. We'd eliminate `crypto.rs` and most of `transport.rs`, and gain forward secrecy + connection migration for free.

**Tradeoff:** More complex dependency, UDP-based (some corporate firewalls block), ~2MB binary size increase.

**Recommendation:** Seriously consider QUIC for v0.2. The current implementation works for proving the concept on LAN; QUIC is the production transport.

### 4. 🟡 JSON Wire Format vs. "Efficiency-First" Claim

JSON is ~2-3x larger than binary alternatives for structured data. For a protocol that claims efficiency as a core principle:
- msgpack: ~40% smaller, trivial to swap (serde compatible)
- protobuf: schema-enforced, more work to adopt
- flatbuffers: zero-copy, overkill for our message sizes

**Recommendation:** msgpack is the easy win — same serde derive macros, just swap the serializer. But JSON is fine for v0.1; our messages are <1KB and bandwidth is not the bottleneck on LAN. Context window tokens are the real "efficiency" concern, and that's an application-layer issue.

### 5. 🟡 TCP Mesh Doesn't Scale

n agents = n(n-1)/2 TCP connections. At 2 agents: 1 connection. At 10: 45. At 100: 4,950. This is fine for our use case (2-3 agents) but architecturally limiting.

**Fix for later:** Star topology with a lightweight relay for >5 agents. Or gossip protocol. Not needed now but worth abstracting the transport layer so we can swap topologies without rewriting.

### 6. 🟢 Missing Features (Not Urgent But Note Them)

- **Graceful shutdown:** SIGTERM handler that drains pending messages before exit
- **Message ordering:** Currently undefined. TCP guarantees per-connection ordering, but if we reconnect, messages could interleave
- **Backpressure:** If one agent floods another, the receiver has no way to signal "slow down"
- **Hot reload:** Config changes require daemon restart
- **Metrics:** No observability (message count, latency, connection health)

### 7. 🟢 Unix Socket IPC — Fine For Now

Not portable to Windows, but our agents run macOS. Linux also supports Unix sockets. Windows named pipes would be the equivalent. Abstract the IPC layer if we ever need Windows support, but don't over-engineer now.

## What Survives a Rewrite?

| Component | Survives? | Notes |
|-----------|-----------|-------|
| mDNS discovery | ✅ as one backend | Add pluggable discovery trait |
| X25519 identity | ✅ for authentication | But not for encryption (use ephemeral + QUIC) |
| Message envelope format | ✅ | JSON or msgpack, same schema |
| Unix socket IPC | ✅ | CLI interface stays the same |
| TCP transport | ⚠️ Replace with QUIC | For production; TCP fine for LAN prototype |
| ChaCha20 hand-rolled crypto | ❌ Replace | Let QUIC handle encryption |
| Peer table / state management | ✅ | Independent of transport |

## Recommended v0.2 Architecture

```
Discovery Layer (trait):
  ├── mDNS backend (LAN)
  ├── Static config backend (VPN/Tailscale)
  └── Rendezvous backend (future, internet)

Transport Layer:
  └── QUIC (quinn) — encryption, framing, reliability, connection migration

Identity:
  └── X25519 static keys for authentication (peer verification)
  └── Ephemeral keys per QUIC session (forward secrecy via QUIC)

IPC:
  └── Unix domain socket (unchanged)

Message Format:
  └── msgpack (serde-compatible swap from JSON)
```

## Action Items

1. **Now:** Add static peer config alongside mDNS (trivial, unblocks Tailscale use)
2. **Now:** Add SIGTERM graceful shutdown
3. **v0.2:** Evaluate quinn QUIC — prototype branch
4. **v0.2:** Swap JSON → msgpack on the wire
5. **Later:** Discovery trait abstraction
6. **Later:** Relay/star topology for >5 agents
