# Security Policy

## Supported Versions

LanBridge is currently pre-1.0. Security fixes are applied to the latest `main` branch and to the latest release builds when releases are available.

## Reporting a Vulnerability

Please do not disclose security vulnerabilities publicly before a fix is available.

Use GitHub's [private vulnerability reporting](https://github.com/kanice888-max/LanBridge/security/advisories/new)
form. If you are unsure whether a behavior is a security issue, report it privately first.

When reporting a vulnerability, include:

- LanBridge version and platform.
- Whether both devices were on the same trusted local network.
- A short reproduction path.
- Logs only when they do not include private file contents, device secrets, or personal data.

## Security Model

LanBridge is designed for trusted local networks. Discovery makes nearby devices visible, but discovery does not imply trust.

The sync model is Primary/Secondary:

- Primary changes sync to Secondary.
- Secondary changes require explicit return-sync.
- Task invites require user acceptance before a task root is registered.
- Conflicts require an explicit user choice.
- Confirmed Primary overwrites must preserve backup semantics.

LanBridge should not be described as fully bidirectional sync.

## File Safety

Deleting a project removes task configuration only. It does not delete local synchronized files.

Primary delete operations move Secondary content into LanBridge history before removal. Conflict resolution and restore operations must keep user-visible states and errors.

## Device Identity

LanBridge stores a local device identity key in the current user's application data directory. Do not share this file. Logs and diagnostics must not contain private key material.

Deleting the identity key creates a new device identity and existing pairings may need to be recreated.

## Local Network Use

Avoid running LanBridge discovery on untrusted public networks unless you understand that device name, device identity, public key, and listening port may be visible to devices on that network.
