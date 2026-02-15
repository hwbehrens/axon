# Security Policy

AXON is a security-sensitive project — its core purpose is authenticated, encrypted agent-to-agent messaging. We take vulnerability reports seriously.

## Supported Versions

| Version | Supported |
|---------|-----------|
| `main` (HEAD) | ✅ |
| Pre-release (`< 1.0`) | ✅ Best-effort |

Until a stable release, security fixes land on `main` directly.

## Reporting a Vulnerability

**Please do not open a public issue for security vulnerabilities.**

Use [GitHub Private Vulnerability Reporting](https://github.com/hwbehrens/axon/security/advisories/new) to submit a report. This keeps the details confidential while we work on a fix.

In your report, please include:

- A description of the vulnerability and its potential impact
- Steps to reproduce or a proof of concept
- The component affected (see scope below)
- Any suggested fix, if you have one

We aim to acknowledge reports within **48 hours** and provide a fix or mitigation within **90 days**. We'll coordinate disclosure timing with you.

## Scope

The following areas are in scope for security reports:

| Component | Examples |
|-----------|----------|
| **Identity & key handling** | Ed25519 key generation, agent ID derivation, key material leakage |
| **Transport (QUIC/TLS)** | TLS 1.3 configuration, certificate validation, handshake bypasses |
| **Peer pinning & authentication** | Accepting unpinned peers, identity/certificate mismatches |
| **Hello-first gating** | Sending or processing application messages before handshake completes |
| **Replay protection** | UUID deduplication bypass, replay cache eviction attacks |
| **IPC** | Unix socket permission issues, command injection via IPC protocol |
| **mDNS discovery** | Spoofed announcements leading to peer impersonation |
| **Wire format** | Malformed messages causing panics, memory exhaustion, or undefined behavior |

## Out of Scope

- Local denial-of-service against the Unix socket (requires local access already)
- Bugs in upstream dependencies (report those upstream; let us know if AXON's usage is affected)
- Social engineering
- Attacks requiring physical access to the host machine

## Acknowledgments

We're happy to credit reporters in release notes and the advisory. Let us know your preference when reporting.
