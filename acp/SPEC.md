# ACP — Agent Communication Protocol Specification

## Overview

ACP is a lightweight daemon for secure, efficient agent-to-agent messaging on a local network. It runs as a background process on each agent's machine.

## Architecture

```
OpenClaw ←→ [Unix Socket] ←→ ACP Daemon ←→ [TCP/mDNS] ←→ ACP Daemon ←→ [Unix Socket] ←→ OpenClaw
```

## Components

### 1. Discovery (`discovery.rs`)

- Advertise via mDNS: service type `_acp._tcp.local`
- TXT records: `agent_id=<id>`, `pubkey=<base64 X25519 public key>`
- Browse for other `_acp._tcp.local` services on the network
- Maintain a peer table: `HashMap<AgentId, PeerInfo>` where PeerInfo = { addr, port, public_key, status, last_seen }
- Re-advertise on startup; remove stale peers after 60s without mDNS refresh
- Use `mdns-sd` crate

### 2. Crypto (`crypto.rs`)

**Key Generation:**
- On first run, generate X25519 static keypair
- Store private key at `~/.acp/identity.key` (chmod 600)
- Public key advertised via mDNS TXT record

**Handshake (on first connection to a new peer):**
1. Both sides already know each other's public keys from mDNS
2. Perform X25519 Diffie-Hellman: shared_secret = DH(my_private, their_public)
3. Derive symmetric key: HKDF-SHA256(shared_secret, salt="acp-v1", info="encryption")
4. Store derived key in peer table; use for all subsequent messages
5. Key lifetime: until either daemon restarts (re-handshake on reconnect)

**Encryption:**
- ChaCha20-Poly1305 AEAD
- 12-byte random nonce per message (prepended to ciphertext)
- Associated data: envelope header (from, to, timestamp) — authenticated but not encrypted

### 3. Protocol (`protocol.rs`)

**Envelope format (JSON, then encrypted):**
```json
{
  "v": 1,
  "id": "<uuid>",
  "from": "<agent_id>",
  "to": "<agent_id>",
  "ts": <unix_millis>,
  "kind": "query|response|delegate|notify",
  "reply_to": null,
  "payload": { ... }
}
```

**Wire format:**
```
[4 bytes: payload length (big-endian u32)]
[12 bytes: nonce]
[N bytes: ChaCha20-Poly1305 ciphertext of JSON envelope]
[16 bytes: Poly1305 tag (appended by AEAD)]
```

Max message size: 64KB (plenty for structured LLM messages).

**Payload kinds:**

- `query`: { question: string, domain?: string, context_budget: "minimal"|"standard"|"full" }
- `response`: { data: any, summary?: string, error?: string }
- `delegate`: { task: string, context?: object, priority: "normal"|"urgent", report_back: bool }
- `notify`: { topic: string, data: any }

### 4. Transport (`transport.rs`)

**TCP Server:**
- Listen on a configurable port (default: 7100)
- Accept connections from discovered peers
- One persistent TCP connection per peer (reconnect on drop)
- Length-prefixed messages (4-byte big-endian u32 + encrypted payload)
- Keepalive: TCP keepalive enabled, 30s interval

**Connection lifecycle:**
1. Peer discovered via mDNS
2. Lower agent_id initiates TCP connection (deterministic, avoids duplicate connections)
3. Both sides derive symmetric key from DH
4. Messages flow bidirectionally
5. On disconnect: attempt reconnect with exponential backoff (1s, 2s, 4s, ... max 30s)

### 5. Local IPC (`socket.rs`)

**Unix Domain Socket:**
- Path: `/tmp/acp-<agent_id>.sock` (or `~/.acp/acp.sock`)
- OpenClaw (or CLI) connects to send/receive
- Simple line-delimited JSON protocol:

**Commands from client (OpenClaw → daemon):**
```json
{"cmd": "send", "to": "agent-two", "envelope": { ... }}
{"cmd": "peers"}
{"cmd": "status"}
```

**Responses from daemon:**
```json
{"ok": true, "msg_id": "..."}
{"ok": true, "peers": [{"id": "agent-two", "addr": "172.16.3.164", "status": "connected"}]}
{"ok": true, "uptime": 3600, "peers_connected": 1, "messages_sent": 42}
```

**Inbound messages (daemon → client):**
```json
{"inbound": true, "envelope": { ... }}
```

### 6. CLI (`main.rs`)

Using `clap` derive:

```
acp daemon [--port 7100] [--agent-id <id>]    # run the daemon
acp send <agent_id> <message>                   # send a quick message (query kind)
acp delegate <agent_id> <task>                  # delegate a task
acp notify <topic> <data>                       # broadcast notification
acp peers                                       # list discovered peers
acp status                                      # daemon health
```

## File Layout

```
~/.acp/
├── identity.key        # X25519 private key (chmod 600)
├── identity.pub        # X25519 public key (base64)
├── config.toml         # optional: port, agent_id override
├── peers.json          # cached peer info (survives restart for faster reconnect)
└── acp.sock            # Unix domain socket
```

## Non-Goals (v0.1)

- No shared state board (may add later)
- No message persistence/history (daemon is stateless; OpenClaw handles history)
- No multi-hop routing (direct peer-to-peer only)
- No TLS (encryption handled at application layer via ChaCha20)
- No file transfer (messages only; use shared filesystem for large data)

## Success Criteria

1. Two ACP daemons on the same LAN discover each other within 5 seconds
2. Messages delivered in <10ms (LAN latency)
3. All messages encrypted end-to-end
4. Daemon uses <5MB RSS memory
5. Clean reconnect after network interruption
6. `acp send` CLI works for quick testing
