# OSS-Fuzz integration (prepared, not applied)

OSS-Fuzz builds a project from its public repository, so this
can only be applied after the repository is public. Applying (the pull
request to `google/oss-fuzz`, its contact and its repository URL) is an
outward-facing step and the maintainer's.

## What is here

- `coreav/`: the OSS-Fuzz project directory, to be copied as
  `projects/coreav/` in a checkout of `google/oss-fuzz`:
  - `project.yaml`: Rust, libFuzzer, AddressSanitizer. `main_repo` is the
    public repository; `primary_contact` is the security address,
    `security@coreav.dev` (`SECURITY.md`); `homepage` is a placeholder on
    `coreav.dev`, which the maintainer sets when applying.
  - `Dockerfile`: the OSS-Fuzz Rust builder image (a nightly Rust and
    `cargo-fuzz`) and a clone of the public repository.
  - `build.sh`: builds every fuzz target with
    `cargo fuzz build -O --debug-assertions` into `$OUT`.
- `isobmff.dict`, `ebml.dict`: libFuzzer dictionaries.
  - `isobmff.dict`: ISO base media box types and sample entry codes, for the
    `coreav-isobmff` and `coreav-isobmff-write` targets.
  - `ebml.dict`: EBML and Matroska element IDs, doc types and codec IDs
    (RFC 8794, RFC 9559), for the `coreav-mkv` targets and
    `isobmff_write_mkv_remux`, which reads Matroska.
  - `text_subtitles` and the WebAssembly target read MP4 and get
    `isobmff.dict`; `policy_check` and `text_sample` get none.
  - They stay in this repository; `build.sh` copies them from the checkout.

## Which targets

`build.sh` takes the list from `scripts/fuzz_plan.py --all`: every
`[[bin]]` target of every `crates/*/fuzz` project. CI's fuzz job and the
fuzz campaigns (`tools/fuzz-campaign/campaign.py`) use the same list, so a
new target is fuzzed everywhere without editing this directory. That was
21 targets when this was written. OSS-Fuzz names each one
`CRATE_TARGET`, the crate without its `coreav-` prefix and with dashes as
underscores (`isobmff_probe`, `mkv_probe`, `isobmff_write_clip`): two crates
both have `probe` and `packets` targets, and `$OUT` is one directory.

## Checking it locally

The check uses OSS-Fuzz's own helper, against a throwaway copy of this
repository: the build runs as root in a container with the copy mounted,
and leaves root-owned `crates/*/fuzz/target/` directories and updated fuzz
`Cargo.lock` files in it. It needs Docker, and nothing is sent anywhere.

```sh
git -C COREAV worktree add --detach /tmp/coreav-ossfuzz   # the throwaway copy
git clone --depth 1 --filter=blob:none --sparse https://github.com/google/oss-fuzz
cd oss-fuzz && git sparse-checkout set infra
mkdir -p projects && cp -r COREAV/tools/oss-fuzz/coreav projects/coreav
# Until the repository is public, the image cannot clone it: build an empty
# source directory instead, and mount the throwaway copy over it.
sed -i 's|^RUN git clone .*|RUN mkdir -p coreav|' projects/coreav/Dockerfile
python3 infra/helper.py build_image --pull coreav
python3 infra/helper.py build_fuzzers --sanitizer address --engine libfuzzer coreav /tmp/coreav-ossfuzz
python3 infra/helper.py check_build coreav
python3 infra/helper.py run_fuzzer coreav isobmff_probe -- -max_total_time=30
```

Afterwards, remove the copy with `sudo rm -rf /tmp/coreav-ossfuzz` and
`git -C COREAV worktree prune`: the build leaves root-owned directories
in it.

`COREAV` is the path of a checkout of this repository. OSS-Fuzz's
`infra/presubmit.py` also checks the project directory; today it reports
the license headers of step 1 as missing, and nothing else.

## Applying it, after the public reset (the maintainer's step)

1. Set `main_repo` in `project.yaml` and the clone URL in the `Dockerfile`
   to the public repository, and check that `primary_contact`
   (`security@coreav.dev`) is tied to a Google account, which OSS-Fuzz
   requires of the address that receives the reports. Add
   `auto_ccs` for others who should see them. Add the Apache License 2.0
   header OSS-Fuzz requires on every file of the project directory
   (`build.sh` and `Dockerfile`; its presubmit looks for
   `http://www.apache.org/licenses/LICENSE-2.0`), with the copyright line
   the maintainer chooses.
2. Run the local check above without the `sed` line, so the image clones
   the public repository.
3. Open the pull request to `google/oss-fuzz` with `projects/coreav/`.
   OSS-Fuzz's new project guide describes the review; a project is
   accepted when it has a significant user base or is critical to
   infrastructure.
4. When it is accepted, link the project's OSS-Fuzz page from `SECURITY.md`.
   Fixes then follow `SECURITY.md`. OSS-Fuzz keeps a report private until
   the fix is released or 90 days have passed, whichever comes first.

Not here yet: seed corpora. The fixtures and the hostile suite are
generated with FFmpeg (`scripts/gen-fixtures.sh`,
`tools/hostile/generate.py`), which the OSS-Fuzz builder does
not run; seeding would mean generating them in the image or committing a
small set of seed files, a choice for when the project is accepted.
