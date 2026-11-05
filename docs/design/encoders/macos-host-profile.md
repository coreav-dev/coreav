# Observed macOS reference profile

This plan precedes the OS-profile implementation and every encoder or metric run on the observed reference host. It amends the operating system of `ENC-MAC-1` to macOS 15.8.2, build 24H232, on the same dedicated Mac mini M1-M. Results apply to 15.8.2, not 15.7 or another OS build. Public metadata uses the fixed neutral provider label `private-reference`; the actual supplier receipt remains private behind its allocation digest.

## Why the OS profile changes

The provider's explicit Sequoia image installed 15.8.2. The observed hardware is Macmini9,1, Apple M1, eight physical CPU cores and 8,589,934,592 memory bytes, with the hypervisor flag zero.

Apple's `softwareupdate --list-full-installers` listed Sequoia 15.8.2, 15.8.1, 15.8 and 15.7.9, but not exact 15.7. The explicit `softwareupdate --fetch-full-installer --full-installer-version 15.7` request exited 1 with `Install failed with error: Update not found`. No replacement installer was downloaded or installed. These observations establish the available provider image and the failed exact-version request, not universal unavailability from every possible source.

## Amendment boundary

Change the exact OS value and redact the nominal provider label consistently in the registration profile and frozen host profile. Describe only hardware in the reference-host prose. Update only the four tool identities below and the top-level plan digest. Keep every native identity, source and raw-frame identity, fixture, command, preparation record, setting and numerical gate unchanged.

The host-fingerprint regression matches every other observed field, including the registered OS build, and checks a matching-host positive control. Its mutation control neutralizes the fingerprint comparison and requires the original regression to fail at a guarded file-hash boundary. No production comparison or validation exception changes.

| Frozen file | Previous SHA-256 | Amended SHA-256 |
|---|---|---|
| `docs/design/encoders.md` | `7e493e1dd0edeca9d504cb6698fa83ef95c4799b971c5489eba69e03b24344df` | `212847fadf20d8bdbca769ee8e3632e6ce7490514f8de6b686473cc733a06490` |
| `scripts/encoders/macos/registration.py` | `5659f2603000a2ebdc09951b91b14459c7e936b89343e19c5a62860c37024ecc` | `fa54f2dd910be4174d6f46503349b75d4d09b4d695700fc57c5f7c8bd494bac4` |
| `scripts/encoders/macos/test_producer.py` | `94effb5744e266625da8dfa76184146af5490b8d74229526b962cf669e92785a` | `23213fc74b24587fd9157d6367238d5230009b323de7f40b73c9ac58c7a21296` |
| `scripts/encoders/macos/videotoolbox-plan.md` | `1f953603c87390c92fef966979b72d17b1aedf5541012fa4f4947dbcf475b3a6` | `d459d5ac4f58e9fc7c96c5654a5b2f5dd8d703a9d61d496c5f683121b7d0bfe0` |

The top-level `plan_sha256` changes from `1f953603c87390c92fef966979b72d17b1aedf5541012fa4f4947dbcf475b3a6` to `d459d5ac4f58e9fc7c96c5654a5b2f5dd8d703a9d61d496c5f683121b7d0bfe0`. The original preparation-verification plan identity remains historical and unchanged.

The source-freeze file changes from SHA-256 `c3deecab4812c69580e93cef5c034d426d9b23adc68faeead213217ed48c8f16` to `77c65a0322892734228b09480f8a8ce6648d3d780a61106d47bb5ecf4541a66d`. Its only changed top-level fields are `host_profile`, `tools` and `plan_sha256`. Within `host_profile`, only `os_version` and the nominal public `provider` label change. All 129 native identities and all other fields remain identical.

## Observed facts and ordered seal

Use the reviewed `registration.host_observation` collector without changing its collection logic. Retain only its bounded host facts and opaque allocation and hardware digests in the public host record. Keep provider identifiers and access details private. Include the installer list and exact-version failure as bounded text evidence.

Merge this profile amendment and its actual host facts first. The final registration seal then names that actual merged revision for the amended source freeze and tooling. This order preserves the existing committed-file and main-ancestry checks without a self-reference to a future squash-merge commit. Both changes receive independent review. No encoder or metric runs before the final seal merges.

The reviewed baseline and observer receipts remain those merged in coreav-dev/coreav#362. Both exporter build receipts remain those merged in coreav-dev/coreav#358. This amendment changes none of their bytes or executable identities, and does not establish encoder availability, conformance, quality or speed.

## Captured host facts

The unchanged reviewed collector observed the physical host with empty diagnostic output. The [public host record](macos-builds/host.json) preserves those facts and opaque digests, but redacts the nominal provider label to `private-reference`. Its hardware digest comes from the collector's domain-separated hash of the observed platform UUID. The allocation digest binds the unmodified private supplier receipt. Neither digest is a guessed identifier or a replacement for the later same-host checks.

The public host record's SHA-256 is `0264f7ac6c3d18fa00161104bc008c1b94ec438351de854bc76861bf2518169f`. The original capture stays private; only its nominal provider field differs from the public projection. All actual host facts and both opaque digests are unchanged.

The [installer evidence](macos-builds/installer-availability.txt) preserves the Sequoia entries from the successful listing and the failed exact-version request. No encoder, raw input or quality metric was used to collect either record. The final seal must bind these exact host facts and this amendment's actual merged revision before judged execution.
