# ADR 0006: Identity trust, key rotation and player-count attestation

Status: accepted (2026-10-18)

## Context

The engine supports pre-shared session keys, optional Ed25519 client identities with a server
allowlist, signed `Welcome` messages and server identity pinning. Competitive deployments need
key rotation/revocation and a story for accounts, reputation and player-count claims. Building a
global account service in-engine would add a network dependency to the authoritative server and
break plaintext LAN/token-only modes.

## Decision

**In-engine trust boundary.** The dedicated server owns a local allowlist of Ed25519 client
public keys and an optional pinned server key. Operators can rotate a key (`OLD:NEW`) or revoke
one at startup/configuration time. Revoking the last key keeps identity enforcement on (all
clients are rejected) instead of silently disabling checks. Half-open or mid-session revocation
is not supported; rotate/revoke before the next match or restart the server.

**External services.** Account identity, reputation, publisher key transparency and revocation
feeds are deployment services, not engine dependencies. Integrators map account IDs to the
public keys in the server allowlist. A transparency/revocation feed is an ordered list of
`timestamp, publisher, key, action(revoke|rotate)` records; servers import the list into their
local allowlist. No network fetch is performed by `moba-net`.

**Player-count attestation (optional, flagged).** A competitive server may sign a per-match
counters attestation with its pinned server key: match id, start/end tick, peak concurrent
players, spectator peak, content hash, and a digest of accepted client identity keys. The
attestation is published alongside results; clients/operators can verify it against the pinned
server key. It proves what the server *claims*, not independent headcount; it is off by default
and never required for LAN/token-only modes.

## Failure modes

- Lost/rotated key → clients with the old key are rejected at `Hello`; operators distribute the
  new key out of band.
- Revocation feed unavailable → server keeps its last local allowlist; it never fails open.
- Compromised server key → pin a new key; old `Welcome` signatures stop verifying.
- Attestation key compromise → attestation is advisory; reputation services must weigh it.
- Plaintext/token-only modes → identity requirements stay disabled unless explicitly configured.

## Consequences

- Rotation/revocation is testable locally and has no runtime network dependency.
- Accounts/reputation remain integration work; the ADR fixes the boundary so the server cannot
  accidentally become an account authority.
- Attestation design is documented but optional; implementing it can land independently.
