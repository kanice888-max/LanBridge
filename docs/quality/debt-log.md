# Debt Log

Track known risks here instead of relying on chat history.

| Date | Area | Debt | Risk | Next Step |
| --- | --- | --- | --- | --- |
| 2026-05-14 | Auto sync | Primary auto-sync currently uses Dashboard polling rather than a process-wide OS watcher. | Auto-sync may pause when the Dashboard is not mounted. | Implement backend-managed watcher or scheduler shared by both platforms. |
| 2026-05-14 | Retry | Retryable transfer failures are surfaced, but durable retry queue/replay is still limited. | Network/VPN changes can require another user action. | Add persistent outbound queue with backoff after minimal flow stabilizes. |
| 2026-05-18 | Packaging | macOS development DMG exists, but signing/notarization and install-after-package smoke validation are not complete. | A locally built DMG can still fail Gatekeeper, permission, or first-run network behavior on another machine. | Run packaged-app smoke tests on real macOS and Windows devices; decide signing/notarization path before external distribution. |
