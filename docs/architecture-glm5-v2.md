I recommend an architecture based on **QUIC (via `quinn`)** for transport, **mDNS/DNS-SD** for discovery, and **Ed25519 self-signed certificates** for security. This combination provides the best balance of speed, security, and robustness for a small-scale LAN agent mesh.

### Architecture Overview

The system consists of a single binary daemon per machine. It operates as a background process that manages a peer-to-peer mesh overlay.

```text
+-------------------+       +-------------------+
|  Agent Process A  |       |  Agent Process B  |
| (OpenClaw Client) |       | (OpenClaw Client) |
+--------+----------+       +--------+----------+
         | Unix Socket (IPC)          | Unix Socket (IPC)
+--------v----------+       +--------v----------+
|   Agent Daemon    |<----->|   Agent Daemon    |
| (QUIC/mDNS/Task)  |  QUIC | (QUIC/mDNS/Task)  |
+-------------------+  UDP  +-------------------+
```

---

### 1. Transport: QUIC (UDP + TLS 1.3)
**Choice:** Use the `quinn` crate (Rust implementation of QUIC) over Tokio.

**Why not TCP?**
TCP introduces head-of-line blocking. If a packet drops, subsequent packets wait. QUIC solves this with independent streams. For agents, this means a large query response won't block a high-priority "abort" signal on the same connection.

**Why not pure UDP?**
Implementing reliability, congestion control, and encryption manually is error-prone. QUIC gives you "TCP reliability" with "UDP speed" and mandatory encryption.

**Performance:** QUIC supports **0-RTT** (Zero Round Trip Time) connection establishment. If Agent A has talked to Agent B before, it can send encrypted data immediately upon handshake, crucial for the sub-10ms requirement.

### 2. Discovery: mDNS/DNS-SD
**Choice:** Use `mdns-sd` or `libmdns` crate.

Agents broadcast their presence on the local network.
- **Service Type:** `_agent-msg._udp`
- **Txt Records:** Contains the Agent's ID (fingerprint of public key) and port.

**Process:**
1. Daemon starts on port 0 (OS assigns random free port) or fixed port.
2. Daemon broadcasts via mDNS: "I am Agent `0xABC...` on port `9999`".
3. Other daemons listen for mDNS events and update their local peer map.
4. **No configuration files needed.** If an agent restarts on a new port, mDNS updates automatically.

### 3. Security: Self-Signed PKI with Public Key Pinning
**Choice:** Ed25519 keys.

Since we cannot rely on a central Certificate Authority (CA) in a zero-config LAN:
1. **Key Generation:** On first run, each daemon generates an Ed25519 key pair.
2. **Identity:** The Agent ID is the SHA-256 hash of the public key.
3. **TLS Integration:** The daemon creates a self-signed X.509 certificate from this key for the QUIC handshake.
4. **Authentication:**
    - mDNS broadcasts the Agent ID.
    - When connecting via QUIC, the client verifies that the peer's certificate public key matches the Agent ID discovered via mDNS.

This prevents Man-in-the-Middle (MitM) attacks even on untrusted LANs, assuming the first mDNS discovery is legitimate (or verified via a side-channel/tofu).

### 4. IPC: Unix Domain Sockets
**Choice:** `tokio::net::UnixStream` with length-delimited frames.

The daemon listens on a local Unix socket (e.g., `/tmp/agent_daemon.sock`).
- **Protocol:** A simple binary protocol:
  - `[u32 length prefix] [JSON Payload]`
- **Why:** High performance (bypasses network stack), standard file permissions (OS level security).

### 5. Message Format
**Choice:** JSON over QUIC Streams.

While binary formats (Protobuf/CBOR) are smaller, JSON is native to LLMs.
- **Envelope:**
  ```json
  {
    "id": "uuid-v4",
    "target_agent_id": "sha256-pubkey",
    "type": "task_delegation",
    "payload": { ... arbitrary structured data ... }
  }
  ```
- **Transport:** Each message opens a new **unidirectional QUIC stream**.
  - *Tradeoff:* Opening a stream has minor overhead, but it prevents head-of-line blocking entirely. For <1KB messages, this is effectively instant.

---

### Implementation Details

#### Dependencies (`Cargo.toml`)
```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
quinn = "0.10"
rcgen = "0.11"        # For generating certificates
mdns-sd = "0.10"      # Discovery
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"         # For fingerprinting keys
bytes = "1"
tracing = "0.1"       # Logging
```

#### Core Daemon Logic (Pseudo-Rust)

```rust
use quinn::{Endpoint, RecvStream, SendStream};
use std::net::SocketAddr;
use tokio::net::UnixStream;

/// The main Agent Daemon State
struct AgentDaemon {
    // QUIC Endpoint handles sending/receiving
    endpoint: Endpoint,
    // Map of AgentID -> SocketAddr (populated by mDNS)
    peers: DashMap<AgentId, SocketAddr>,
    // Local identity
    identity: Identity,
}

/// Handles incoming IPC from local OpenClaw/Agent process
async fn handle_ipc_client(mut stream: UnixStream, daemon: Arc<AgentDaemon>) {
    // 1. Read length-prefixed JSON from Unix socket
    let msg: AgentMessage = read_json_from_socket(&mut stream).await;
    
    // 2. Look up destination IP via mDNS cache
    let target_addr = daemon.peers.get(&msg.target_agent_id).unwrap();
    
    // 3. Open QUIC connection (0-RTT if possible)
    let conn = daemon.endpoint.connect(target_addr, "agent-mesh")?.await?;
    
    // 4. Open a stream and send
    let mut send_stream = conn.open_uni().await?;
    send_stream.write_all(&serde_json::to_vec(&msg)?).await?;
    send_stream.finish().await;
}

/// Handles incoming QUIC connections from other Agents
async fn handle_quic_inbound(conn: quinn::Incoming, daemon: Arc<AgentDaemon>) {
    let conn = conn.await?;
    // Verify peer ID matches certificate
    let peer_id = validate_peer_cert(&conn)?;
    
    // Accept streams indefinitely
    while let Ok(stream) = conn.accept_uni().await {
        let data = stream.read_to_end(1024).await?; // Max 1KB
        // Push to local IPC or handle internally
        forward_to_local_agent(data).await;
    }
}
```

### Tradeoffs & Mitigations

| Tradeoff | Discussion |
| :--- | :--- |
| **UDP on LAN** | Some restrictive corporate networks block UDP. **Mitigation:** The architecture allows a TCP fallback transport, but for the default "opinionated" path, QUIC is superior. |
| **mDNS Reliability** | mDNS can be flaky on complex VLANs or with WiFi power saving. **Mitigation:** Include a "Known Hosts" cache file. If mDNS fails, the daemon attempts to connect to the last known IP. |
| **JSON Overhead** | Parsing JSON is slower than Bincode. **Mitigation:** At <1KB and local LAN speeds, the network latency (0-5ms) dominates CPU parsing time (<0.1ms). The interoperability gain is worth the microseconds. |
| **0-RTT Replay Attacks** | QUIC 0-RTT data can theoretically be replayed. **Mitigation:** The application layer should include a nonce/timestamp in the message payload to detect duplicates. |

### Scalability Path (Internet Mode)
To move off LAN later:
1. **Discovery:** Swap `mdns` for a REST-based "Rendezvous Server" or DHT.
2. **NAT Traversal:** Enable QUIC's built-in connection migration. Add a STUN client to determine public IP/port.
3. **No architecture change:** The Agent ID remains the cryptographic identifier; only the transport address (SocketAddr) changes from LAN IP to Public IP.