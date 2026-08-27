pub fn print_annotated_examples() {
    println!(
        r#"AXON — Complete annotated example interactions
==============================================

LLMs learn from examples faster than from specifications.
Below is a full request → response and fire-and-forget messaging sequence.

Agent IDs used:
  Alice: ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4
  Bob:   ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3

Intentional peer enrollment
──────────────────────────────────────────────
Bonjour discovers LAN candidates, but discovery does not grant trust:
  1) Alice runs: axon peers
  2) Alice runs: axon connect ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3

When Bonjour is unavailable, use a token with a stable locator:
  1) Bob runs:   axon identity
  2) Alice runs: axon connect axon://<pubkey_base64url>@192.168.1.42:7100

Network Protocol (QUIC)
──────────────────────────────────────────────
The following steps show the network-level QUIC protocol interaction.

──────────────────────────────────────────────
Step 0: Start the daemon
──────────────────────────────────────────────
$ axon daemon --port 7100

  INFO starting AXON daemon agent_id=ed25519.a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4 port=7100

  (The daemon binds QUIC on 0.0.0.0:7100, creates ~/.axon/axon.sock for IPC,
   discovers LAN candidates, and reconnects intentionally enrolled peers.)

Verbosity levels (choose based on workload):
  $ axon -q  daemon   # warn only — best for high-throughput LLM relay
  $ axon    daemon    # info (default) — logs each inbound message summary
  $ axon -v  daemon   # debug — includes truncated payload previews (256 bytes)
  $ axon -vv daemon   # trace — full untruncated payloads

──────────────────────────────────────────────
Step 1: List known peers
──────────────────────────────────────────────
$ axon peers

  IPC sent:     {{"cmd":"peers"}}
  IPC response: {{
    "ok": true,
    "peers": [
      {{
        "agent_id": "ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3",
        "public_key": "<base64>",
        "trust": "enrolled",
        "locators": ["192.168.1.42:7100"],
        "status": "connected",
        "rtt_ms": 1.23
      }}
    ]
  }}

──────────────────────────────────────────────
Step 2: Send a request
──────────────────────────────────────────────
$ axon request ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 "What is the capital of France?"

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"request","payload":{{"message":"What is the capital of France?"}}}}
  Wire message: {{
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "kind": "request",
    "payload": {{"message":"What is the capital of France?"}}
  }}
  Wire response: {{
    "id": "660e8400-e29b-41d4-a716-446655440001",
    "kind": "error",
    "ref": "550e8400-e29b-41d4-a716-446655440000",
    "payload": {{"code":"unhandled","message":"no application handler registered for request '550e8400-e29b-41d4-a716-446655440000'","retryable":false}}
  }}
  (If the remote agent has an app handler, it may return a normal "response" instead.)

──────────────────────────────────────────────
Step 3: Send a fire-and-forget message
──────────────────────────────────────────────
$ axon notify --json ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3 '{{"state":"ready"}}'

  IPC sent:     {{"cmd":"send","to":"ed25519.f6e5d4c3b2a1f6e5d4c3b2a1f6e5d4c3","kind":"message","payload":{{"data":{{"state":"ready"}}}}}}
  IPC ack:      {{"ok":true,"msg_id":"..."}}
  (No wire response — message is unidirectional / fire-and-forget.)

──────────────────────────────────────────────
IPC Commands — Raw JSON (Unix socket)
──────────────────────────────────────────────

All examples below are newline-delimited JSON sent over ~/.axon/axon.sock.
All connected clients receive fire-and-forget messages as broadcast events.
One client may lease request handling with `serve`.

# 1. Send a request (bidirectional — waits for response)
→ {{"cmd":"send","to":"ed25519.f6e5d4c3...","kind":"request","payload":{{"message":"What is 2+2?"}}}}
← {{"ok":true,"msg_id":"550e8400-...","response":{{"id":"660e8400-...","kind":"error","ref":"550e8400-...","payload":{{"code":"unhandled","message":"no application handler registered for request '550e8400-...'","retryable":false}}}}}}

# 2. Send a fire-and-forget message (unidirectional)
→ {{"cmd":"send","to":"ed25519.f6e5d4c3...","kind":"message","payload":{{"data":{{"state":"ready"}}}}}}
← {{"ok":true,"msg_id":"770e8400-..."}}

# 3. List peers
→ {{"cmd":"peers"}}
← {{"ok":true,"peers":[{{"agent_id":"ed25519.f6e5d4c3...","public_key":"<base64>","trust":"enrolled","locators":["192.168.1.42:7100"],"status":"connected","rtt_ms":1.23}}]}}

# 4. Daemon status
→ {{"cmd":"status"}}
← {{"ok":true,"uptime_secs":3600,"peers_connected":1,"messages_sent":42,"messages_received":38}}

# 5. Daemon identity
→ {{"cmd":"whoami"}}
← {{"ok":true,"agent_id":"ed25519.a1b2...","public_key":"<base64>","name":"my-agent","version":"<version>","uptime_secs":3600}}

# 6. Acquire the single inbound request-handler lease
→ {{"cmd":"serve"}}
← {{"ok":true,"serving":true}}

# 7. Answer a delivered request on its original QUIC stream
← {{"event":"request","request_id":"880e8400-...","from":"ed25519.f6e5d4c3...","envelope":{{"id":"880e8400-...","kind":"request","payload":{{"question":"Hello?"}}}}}}
→ {{"cmd":"reply","request_id":"880e8400-...","kind":"response","payload":{{"answer":"Hi"}}}}
← {{"ok":true,"request_id":"880e8400-..."}}

# 8. Inbound fire-and-forget event (broadcast; lagging clients are disconnected)
← {{"event":"inbound","from":"ed25519.f6e5d4c3...","envelope":{{"id":"990e8400-...","kind":"message","payload":{{"state":"ready"}}}}}}

──────────────────────────────────────────────
Notes
──────────────────────────────────────────────
- Either side can initiate the QUIC connection; duplicates are resolved automatically.
- Unknown peers are rejected during TLS; Bonjour candidates require explicit enrollment.
- Messages are framed by QUIC stream FIN (no length prefix).
- Bidirectional streams are used for request/response patterns (kind: "request").
- Unidirectional streams are used for fire-and-forget messages (kind: "message").
- Identity is established by mTLS — peer identity is derived from the TLS certificate.
"#
    );
}
