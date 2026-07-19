# macOS Installation And Distribution

LanBridge currently publishes its macOS DMG through GitHub with a free ad-hoc code signature,
without a paid Developer ID certificate or Apple notarization. Ad-hoc signing gives each bundle a
complete code signature, but it does not establish an Apple-verified publisher identity.

## User Installation

1. Download the DMG and matching `.sha256` file from the same GitHub Release.
2. Verify the download with `shasum -a 256 LanBridge_*.dmg`.
3. Open the DMG and drag `LanBridge.app` into `/Applications`.
4. On first launch, Control-click or right-click `LanBridge.app`, choose **Open**, then confirm.
5. If macOS still blocks the app, attempt to open it once, then use **System Settings → Privacy
   & Security → Open Anyway**.
6. Grant Desktop/Documents access only when the selected sync task actually uses that protected
   folder. One prompt on first access is expected.
7. Grant **Local Network** access. After replacing an older build, if the app reports `os error 65`,
   turn the permission off and on once, quit LanBridge completely, and reopen it.

Never instruct users to disable Gatekeeper globally. In particular, release notes and support
responses must not recommend `spctl --master-disable`.

## Release Checklist

- Set and commit the release version before packaging. `npm run package:mac` validates that all
  version sources already match and then creates the architecture-specific DMG without changing
  tracked version files.
- Build the Intel and Apple Silicon DMGs manually from the matching Git tag. Upload the DMGs and
  their checksum files to the draft GitHub Release manually; the release workflow does not build
  or upload installer files.
- Keep the bundle identifier stable as `com.lanbridge.app`.
- Package the app at the stable `/Applications/LanBridge.app` path.
- Publish SHA-256 checksums next to every DMG.
- Verify `codesign --verify --deep --strict LanBridge.app`, `Signature=adhoc`, and bundle identifier
  `com.lanbridge.app` before publishing.
- Label the artifact as **ad-hoc signed, not Developer ID signed, and not notarized**.
- Smoke-test on a clean macOS account: manual Gatekeeper approval, first launch, protected-folder
  approval, restart, and a second launch without another prompt for the same build.
- Re-test approval behavior after every upgrade because unsigned builds can require users to
  approve the new artifact again.

## Known Limitations

- Gatekeeper can block the first launch or require **Open Anyway**.
- Managed Macs may prohibit ad-hoc-signed applications entirely.
- Permission grants may not survive an application replacement as reliably as they do for a
  stable Developer ID identity.
- If a real installed build still gets `EHOSTUNREACH` while Terminal reaches the same peer, do not
  mark the issue fixed; Developer ID signing/notarization remains the reliable future option.
