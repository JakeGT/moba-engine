# Security policy

## Preview warning

The default protocol is **plaintext** for LAN development. An optional pre-shared-key mode
authenticates the handshake and encrypts every application packet with ChaCha20-Poly1305:

```bash
# 32-byte key as 64 hex characters
SECRET=$(openssl rand -hex 32)
cargo run -p moba-server -- --listen 0.0.0.0:7777 --seconds 600 --secret "$SECRET"
MOBA_SECRET="$SECRET" cargo run -p moba-client
```

This protects payload confidentiality/integrity and proves key possession to the server. It
is **not** a complete public-internet security model:

- it is a shared key, not per-player identity, and has no forward secrecy;
- packet metadata (addresses, sizes, timing, session ids) is visible;
- per-address UDP budgets reject floods of valid packets, but malformed-packet bandwidth and
  spoofed-source floods still need a network/edge gateway;
- there is no matchmaking identity or revocation;
- replay is limited by sequence/ack semantics, not a formal anti-replay window;
- a compromised client or key compromises that match.

Do not expose a plaintext server to the public internet. For public deployments, terminate
transport security in a trusted gateway or wait for the M7 authenticated key-exchange work.

## Reporting a vulnerability

Please report suspected vulnerabilities privately via the repository's **GitHub Security
Advisories** ("Report a vulnerability" on the Security tab) once the public repository is
available. Include:

- affected crate/version and platform;
- reproduction steps or a packet capture;
- impact assessment (crash, spoofing, denial of service, state corruption).

Maintainers will acknowledge within 7 days and coordinate a fix and disclosure. Please do not
publish details before a fix is available.

## Scope

In scope: parser/AEAD panics or overflows, key/nonce misuse, spoofed-input state corruption,
denial of service via packets, asset/shader tooling issues, unsafe-code soundness.

Out of scope: cheating in unranked/preview matches, missing public identity/anti-DoS features
(documented above), issues in third-party drivers or Vulkan validation layers.
