# Built-in encoders

Status: Design only. No backend, command option, or release feature described here is implemented by this note.

## Static-link size experiment

This experiment measures linked code, not encoding speed or the size of a finished backend. It has no pass threshold. The method is committed before the build.

Build the current `coreav-cli` with `release-default`, `--profile dist`, `x86_64-unknown-linux-musl`, fat LTO, one codegen unit, and stripped symbols. Build a pinned SVT-AV1 release as a static musl library with release optimization and no applications or tests. Relink the same CLI with that archive and explicit linker roots for the documented encoder lifecycle, configuration, input, output, and version functions. Do not add an encoder implementation or invoke those functions. Keep section garbage collection enabled.

Record the base and linked executable sizes in bytes, their difference, the static archive size, both ELF dependency checks, the SVT-AV1 archive hash, compiler versions, build flags, and retained API symbols before stripping. Also record an all-sections-retained link as a conservative comparison. A symbol root must survive into executable code or the experiment fails. Neither result includes the future Rust wrapper, worker protocol, or notices. The final backend must repeat the packaged-size measurement.
