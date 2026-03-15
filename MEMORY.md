# AXON — Project Memory

_Last updated: 2026-03-14_

## Overview
Agent eXchange Over Network — peer-to-peer agent communication protocol.
Repo: github.com/hwbehrens/axon (transferred from obscurity6437/agent-comms)

## Current State
- **Version:** v0.7.2 (Homebrew)
- **Status:** Full bridge pipeline working end-to-end

## Architecture
- Pipeline: daemon → IPC socket → axon-bridge.py → /hooks/agent → OpenClaw → iMessage
- My identity: `ed25519.184327cb9cab59d9fafcfc5b193d43e3`
- Hans identity: `ed25519.993f64086cc10c8d3e39ee0d011f8c8e`
- Connected via static peers
- Hooks config: `hooks.enabled: true`, token: `axon-bridge-hook-token`

## Architecture Simplification
- **Target:** ~40-50% code reduction across 12 phases
- **Phases 1-2:** Complete (Feb 2026) — replay cache + hello handshake removed
- **Phase 3:** Ready and waiting (pending Hans return from London Mar 20)
- Next after Phase 3: proper channel plugin, daemon LaunchAgent

## Key History
- **2026-02-15:** PRs #1 (91/100), #2 (95/100) merged. PR #3 IPC v2 reached 97/100.
- **2026-02-17:** First agent-to-agent message received (v0.5.0). Built axon-bridge.py for IPC→OpenClaw forwarding.
- **2026-02-18:** Upgraded to v0.7.2 — full bridge integration working.

## Known Issues
- Issue #22: Fixed
- Issue #23: Stale peer doctor
- `known_peers.json`: Daemon re-persists in-memory state → edits while running get overwritten. Must stop daemon first.
- Config is YAML (config.yaml), not JSON

## Lessons
- Don't transplant solutions across problem domains. Reticulum's protocol features solve problems caused by its constraints (no TLS, no QUIC, LoRa radios). AXON sits on modern internet infra. The right takeaway is philosophical (identity-first design, sovereignty), not mechanical (copy protocol features).
- Self-review blindness: Internal Judge scored PR #3 at 90/100; external reviewer scored 75/100. 15-point delta.
