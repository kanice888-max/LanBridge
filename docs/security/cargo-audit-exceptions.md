# Cargo Audit Exceptions

The release CI fails on RustSec vulnerabilities by default. The following temporary exceptions
are reviewed before every release.

| Advisory | Dependency path | Reason | Expiry |
| --- | --- | --- | --- |
| `RUSTSEC-2026-0194` | `tauri 1.8.3 → plist 1.9.0 → quick-xml 0.39.4` | Tauri 1 pins `quick-xml ^0.39.2`; the affected parser is used for build-time, project-controlled plist metadata, not LanBridge network or synced-file input. | Upgrade or replace the Tauri 1 stack before 2026-10-19. |
| `RUSTSEC-2026-0195` | `tauri 1.8.3 → plist 1.9.0 → quick-xml 0.39.4` | Same constrained build-time plist parser path. Do not add any untrusted plist input while this exception exists. | Upgrade or replace the Tauri 1 stack before 2026-10-19. |

`crossbeam-epoch` is upgraded through `Cargo.lock` to the patched release. Unmaintained GTK3 and
Tauri 1 transitive warnings remain visible in audit output; they are not treated as vulnerability
exceptions and must be reconsidered during the planned Tauri major-version upgrade.
