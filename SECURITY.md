# Security policy

## Supported versions

Bonhomme is pre-release software. Security fixes are made on the current `main` branch; no released
version line is currently supported. Production operators should pin reviewed commits and follow
the hardening and recovery procedures in [docs/operations.md](docs/operations.md).

## Reporting a vulnerability

Do not disclose suspected vulnerabilities, leaked credentials, tenant data, or exploit details in a
public issue. After the organization transfer, use GitHub private vulnerability reporting for this
repository. Until that channel is enabled, contact the maintainers through the destination
organization's approved private security channel.

Include the affected commit, impact, reproduction conditions, and any known mitigations. Remove
real secrets and personal data from evidence. Maintainers should acknowledge the report privately,
coordinate remediation and disclosure, and credit reporters when requested and appropriate.

If a credential may have escaped, rotate it immediately; do not wait for triage. For operational
containment, audit verification, backup, and recovery, follow [docs/operations.md](docs/operations.md).
