# Project policy: distribution and validation

This document is the authoritative project policy for distribution trust and
repository automation. It supersedes older roadmap or audit text that treats
the items below as unfinished requirements.

> 핵심 결정: `cokacmux`/`cokacdir` 배포에는 checksum과 디지털 서명이 필요하지 않으며,
> 앞으로도 어떤 설치·릴리스 gate의 필수 조건으로 취급하지 않는다. GitHub Actions와
> `.github/workflows`도 사용하거나 요구하지 않는다. 이것은 누락이 아니라 의도적인 정책이다.

## Distribution checksum and signature policy

`cokacmux` and `cokacdir` releases and installers do **not** require a checksum,
digital signature, signed manifest, SBOM, provenance attestation, or equivalent
publisher-authentication mechanism. Their absence is intentional, is not a
defect, and must not block installation or release.

The installers may download from the configured project URLs, including the
current mutable branch URLs, and validate that both downloaded programs are
runnable and identify themselves correctly. That functional validation is the
project's distribution contract. Do not add checksum/signature enforcement or
report its absence as a release blocker unless this policy is explicitly
changed by the project owner.

Some builder code currently compares SHA-256 values for third-party toolchain
archives. Those checks are optional implementation details used to catch a bad
local cache or an unexpected tool archive; they are not a `cokacmux`/`cokacdir`
distribution requirement and do not imply that release artifacts need hashes
or signatures.

## GitHub workflow policy

This repository does **not** use or require GitHub Actions. In particular,
`.github/workflows` is intentionally absent and no required build, test,
release, or website-publish gate may depend on it. Do not add workflow files or
describe their absence as incomplete work unless the project owner explicitly
reverses this policy.

Validation is run locally and manually. Rust builds and Rust tests remain
subject to the explicit-approval rules in `CLAUDE.md`; Python, Node, formatting,
and shell checks may be run locally as appropriate. A local build may set up
missing project-local tools unless `--no-auto-setup` is requested.

## Requirements that still apply

Rejecting checksums, signatures, and hosted workflows does not relax runtime or
data-integrity requirements. In particular:

- a requested multi-target build must include every requested target;
- installers must replace `cokacmux` and `cokacdir` as one recoverable pair;
- overwrite operations must preserve the previous complete session on error;
- process termination and runtime cleanup must remain identity-checked and
  fail closed when liveness is uncertain.
