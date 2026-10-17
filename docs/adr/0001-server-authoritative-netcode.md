# ADR-0001: Server-authoritative netcode

**Status:** accepted (2026-10-14)

## Context
MOBAs have 10 players, hundreds of units and fog of war. They're highly competitive, so map hacks and state tampering must be impossible.

## Decision
- A dedicated server runs the only authoritative simulation at a fixed 30 Hz.
- Clients send inputs (move/attack/cast intents with sequence numbers). The server validates every one of them.
- The server sends each client a delta-compressed snapshot against that client's last acked baseline. It contains **only entities visible to that client's team** (vision is computed on the server).
- The client predicts its own hero (movement, cast start) using the shared `sim` code and reconciles by replaying unacked inputs. It interpolates every other entity about 100 ms behind.
- Skillshot hits use server-side lag compensation (rewind to the shooter's view time, bounded to 200 ms).

## Rejected
- **Deterministic lockstep:** fog of war can't be enforced (every client holds full state), and one slow peer stalls everyone.
- **Hybrid with full determinism:** too costly up front. We still keep the sim deterministic-friendly (clippy guardrails) so replays and desync diagnosis stay cheap.

## Consequences
- Server CPU cost scales with matches, so the 2 ms/tick budget is critical.
- Bandwidth per client must be engineered (quantisation, interest management).
