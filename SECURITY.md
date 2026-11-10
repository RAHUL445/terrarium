# Security policy

Terrarium is an experimental source preview. No version is a stable or
security-certified release. Reports against the current `main` branch are
welcome; fixes are best effort with no guaranteed response time.

## Reporting a vulnerability

Please report privately through GitHub:
**Security → Advisories → Report a vulnerability** on this repository.
Do not open a public issue for security problems.

Include the affected commit and platform, the impact, and a minimal
reproduction using synthetic data. Never include real credentials, recovery
phrases, personal records or logs containing them, and do not test against
other people's devices or data.

## Scope

In scope: the host runtime and plugin sandbox, package signing and install
consent, sync encryption and peer authentication, Tauri IPC exposure, and the
`terra` CLI. Plugin implementations are in
[terrarium-plugins](https://github.com/RAHUL445/terrarium-plugins) and have
their own policy. See the [threat model](docs/security/threat-model.md).

## Known limits

- The full dependency audit reports one glib unsoundness finding
  (RUSTSEC-2024-0429) and five maintenance warnings. None is in the reviewed
  arm64 macOS/Android build graphs, and CI guards that. The blocking audit
  records the glib finding as known so that any new finding fails CI; a
  separate strict-audit job with no exceptions keeps reporting it. This is
  not a risk acceptance. Other architectures and feature sets need separate
  review.
- Blocking a device is host-local. Signed membership and key-rotation
  primitives exist, but complete compromised-device revocation is not active,
  and data already downloaded cannot be erased remotely.
- Retention controls, hard quotas, fuzzing, full backup recovery and
  production-signed artifact verification are unfinished.

Details and acceptance criteria are in the [roadmap](docs/roadmap.md).
Building from source does not produce an app-store or production binary.
