# Windows Installation And Distribution

LanBridge publishes x64 Windows installers through GitHub Releases.

## Install

1. Download the `.exe` installer for a normal personal installation, or the `.msi` package for
   managed or enterprise deployment, from the same GitHub Release as `SHA256SUMS.txt`.
2. In PowerShell, verify the downloaded file with:

   ```powershell
   Get-FileHash .\LanBridge_*.exe -Algorithm SHA256
   ```

3. Compare the reported hash with the matching line in `SHA256SUMS.txt`, then run the installer.
4. Keep Microsoft Edge WebView2 Runtime installed; LanBridge uses it for its desktop interface.
5. Allow LanBridge through the local firewall only on trusted networks when Windows asks.

## Uninstall

Use **Settings → Apps → Installed apps → LanBridge → Uninstall**. Removing the application does
not remove synchronized folders. Remove local application data only if you intentionally want to
discard pairings, tasks, history metadata, and the device identity.

## Release Checklist

- Build x64 installers on a clean Windows environment with `npm run package:win`.
- Verify the installer and matching SHA-256 checksum before upload.
- Smoke-test installation, first launch, pairing, a Primary-to-Secondary transfer, explicit
  return-sync, and uninstall behavior.
