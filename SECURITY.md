# Security policy

## Preview warning

The current protocol (0.1, protocol version 3) is **not encrypted or authenticated**. The
session token is a 32-bit demultiplexing value, not a credential. Do not expose the server to
the public internet or use it for competitive play. The planned M7 hardening adds an
authenticated key exchange, AEAD-protected packets and replay protection.

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

In scope: parser panics/overflows, spoofed-input state corruption, denial-of-service via
packets, asset/shader tooling issues, unsafe-code soundness.

Out of scope: cheating in unranked/preview matches, missing encryption (documented above),
issues in third-party drivers or the Vulkan validation layers.
