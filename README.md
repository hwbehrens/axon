<p align="center">
  <img src="axon/assets/icon.png" alt="AXON" width="128" />
</p>

# AXON — Agent eXchange Over Network

A hyper-efficient, LLM-first local messaging protocol for agent-to-agent communication.

## Problem

Current inter-agent communication options are all designed for humans:
- iMessage: rich media, typing indicators, read receipts — none of which LLMs need
- HTTP/REST: stateless, no session awareness, JSON overhead
- OpenClaw sessions_send: works but goes through the gateway abstraction layer

We want something purpose-built for two LLM agents on the same local network.

## Design Principles

1. **Context-budget-aware**: Every message costs tokens. The protocol should minimize unnecessary context consumption.
2. **Structured-first**: No natural language overhead. Payloads are typed, schemaed, and machine-parseable.
3. **Resumable**: Agents restart frequently. The protocol handles reconnection, deduplication, and state recovery.
4. **Minimal round-trips**: Prefer rich single exchanges over chatty back-and-forth.
5. **Zero-trust locally**: Agents authenticate even on LAN (agents have different access levels).

## Status

Working implementation. The daemon, CLI, IPC, QUIC transport, mDNS discovery, and static peer config are all functional. See `spec/` for the protocol specification.

## Quickstart

### Build

```sh
cd axon
cargo build --release
```

The binary is at `axon/target/release/axon`. Add it to your `PATH` or run it directly.

### Run a single daemon

```sh
axon daemon
```

This starts the daemon on the default port (7100), enables mDNS discovery, creates `~/.axon/` with a fresh Ed25519 identity, and listens for IPC commands on `~/.axon/axon.sock`.

### Connect two agents on a LAN

If both machines are on the same local network, mDNS handles everything:

```sh
# Machine A                          # Machine B
axon daemon                          axon daemon
```

Within seconds they discover each other. Verify with:

```sh
axon peers
```

### Connect two agents over Tailscale/VPN (static peers)

When mDNS won't work (different subnets, VPN, etc.), configure peers manually:

```sh
# On each machine, get the identity:
axon identity
# Output: { "agent_id": "ed25519.a1b2c3d4...", "public_key": "base64..." }
```

Then on machine A, create `~/.axon/config.toml`:

```toml
[[peers]]
agent_id = "ed25519.<machine-B-agent-id>"
addr = "<machine-B-ip>:7100"
pubkey = "<machine-B-public-key>"
```

Do the same on machine B with A's info. Then start both daemons:

```sh
axon daemon --disable-mdns    # optional: skip mDNS if not needed
```

### Send a message

```sh
# Query another agent
axon send <agent_id> "What is the capital of France?"

# Delegate a task
axon delegate <agent_id> "Summarize today's news"

# Fire-and-forget notification
axon notify <agent_id> meta.status '{"state":"ready"}'

# See all commands
axon --help
```

### Discover capabilities

```sh
axon discover <agent_id>
```

### Example interaction

```sh
axon examples    # prints a full annotated hello → discover → query → delegate flow
```

## Documentation

| Document | Description |
|----------|-------------|
| [`spec/SPEC.md`](./spec/SPEC.md) | Protocol architecture (QUIC, Ed25519, discovery, lifecycle) |
| [`spec/MESSAGE_TYPES.md`](./spec/MESSAGE_TYPES.md) | All message kinds, payload schemas, stream mapping |
| [`spec/WIRE_FORMAT.md`](./spec/WIRE_FORMAT.md) | Normative wire format for interoperable implementations |
| [`CONTRIBUTING.md`](./CONTRIBUTING.md) | Development guide, module map, testing requirements |
| [`evaluations/`](./evaluations/) | Agent evaluation rubrics and results (not part of the implementation) |
