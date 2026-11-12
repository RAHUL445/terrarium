# Roadmap

Open work before Terrarium can be called stable or shipped as signed binaries.
Publishing the source does not complete these items. Detailed designs live in
[docs/security](security/).

## Built in the unreleased source

- Global indexed search, item links, Markdown mentions and backlinks.
- Custom page views with tables, calendars and boards; declared card moves.
- Desktop quick capture, page/item deep links and Android text/link sharing.
- Bulk actions and optional undo; readable export and CSV/Markdown import.
- Encrypted attachments, quota and local cleanup, including backup support.
- Plugin events and approved automations on a chosen device.
- Approved plugin HTTPS, encrypted accounts and desktop OAuth sign-in.
- Item History, restore and per-space visibility periods. Erasure is separate.
- Signed catalog browsing and update review, declarative plugins and
  `terra new --declarative`. Production registry activation remains gated.
- Desktop layout, wording, completion toasts, backup fallback and test profiles.

See the [guides](README.md#user-guide) and [changelog](../CHANGELOG.md).
Native Home Screen widgets remain a [proposal](design/home-widgets.md).
JavaScript plugin authoring remains [research](design/js-guest-spike.md).
Registry activation needs the [publishing gates](design/registry.md#27-publishing-and-activation-gates),
including artifact provenance and Android policy review; it is not enabled
by the implementation alone.

## Release gates and deferred work

| ID | Priority / gate | Task and acceptance |
| --- | --- | --- |
| SEC-01 | Before stable/binary security approval | Resolve the glib full-lock unsoundness finding and five maintenance warnings through supported upstream upgrades or separately reviewed source remediation/policy. Keep the strict scan and both arm64 exposure guards; no silent ignores. Independently review the Loro/imbl backport and retain mixed-version fixture proof. |
| SEC-02 | Before claiming compromised-device revocation | Integrate pinned authority, signed enrollment/epochs, durable secret keyring/journal, protocol negotiation, current-writer enforcement, offline distribution, shared-secret resealing, backup/recovery and Drive authorization revocation. Prove crash/failure/mixed-version cases. Current Block device stays explicitly host-local. See [membership v2](security/membership-v2-milestone.md). |
| SEC-03 | Privacy hardening | History retention now hides old versions; real erasure still needs change-log compaction. Add audio/transcript retention controls, crash-residue sweeping, notification-preview preference and complete log review. Decide database-at-rest encryption and loss/recovery behavior; test backup compaction without promising secure erasure. |
| SEC-04 | Hostile-input hardening | Keep hard attachment size and per-space disk quotas enforced, and finish remaining hostile-input work: complete IPC/AI budgets, strict optional-field typing, parser/package/protocol fuzzing and filesystem-race review. Plugin HTTP has persisted attempt/body-byte budgets and bounded activity history; these do not supply general hard disk quotas. The polled Git cap can overshoot and is not a hard network/disk quota. |
| REL-01 | Before public binary distribution | Generate artifact-specific SBOM/license inventory with required dependency/model notices, checksums and provenance. Review signatures, signing-key protection, install/upgrade/recovery and supported OS/architecture matrix. Source-only notices are not a complete binary license report. |
| REL-02 | Before production macOS release | Developer ID sign/notarize/staple; verify Gatekeeper, entitlements, Wasmtime and voice/microphone behavior under the final signature. Current desktop build is a debug/development bundle. |
| REL-03 | Before production Android/store release | Production signing/AAB and signing-key recovery, native library/page-size checks, permission/backup/export review, physical upgrades/background/Doze/notification tests, store Data safety/privacy and downloadable-plugin policy review. Current APK uses the existing development certificate. |
| QA-01 | Accessibility/device quality | Fix native search-palette accessibility exposure; test VoiceOver/TalkBack, landscape, large fonts, alternate themes, small windows and other target architectures. |
| QA-02 | Recovery/protocol evidence | Test actual isolated OS secret-store failure/migration, clean-device recovery, old backups, mixed-device upgrades and full live blocking/rotation without using normal credentials as fixtures. One real-Keychain test is intentionally ignored. |
| PERF-01 | Performance follow-up | Repeat fresh-process/warm-resume Android profiling; investigate startup phases with several samples and representative data. Do not infer a speedup from one sample. |
| OPS-01 | After GitHub creation | Enable private reporting, configure branch protection/required checks and Dependabot review ownership, run hosted CI, and document vulnerability response/release rollback. No production secrets in repository Actions variables. |

Deferred means unfinished, not accepted risk or a waived production gate.
