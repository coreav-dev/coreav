# Local oracle qualification

The FFmpeg integration test must qualify the selected local executable before comparing output or generating fixtures. Executable permissions alone do not prove the shared pin.

## Plan

Use the existing `scripts/oracle_pin.py --verify-bin` boundary for the installed FFmpeg and ffprobe pair. Keep the pin file, version matcher, installer, runtime fallback selection and comparison predicates unchanged.

Require the selected FFmpeg file to resolve to the verified directory's `ffmpeg` file. This prevents a renamed local override from qualifying a different executable in its parent directory. A renamed standalone override without that installed pair is not qualified test evidence.

Share this check between the generated input-seek refusal verifier and the existing native-copy oracle helper. Preserve the existing optional skip when no local pair is installed; a present but unqualified pair fails. CI still requires the pair.

Test a valid pair, a wrong version, a malformed or failed version command, a missing sibling and a different selected executable beside a valid pair. Derive positive fake banners from the shared pin rather than adding another version constant. Test failures must identify the oracle-qualification stage before any fixture output is created.

## Scope

This is test qualification only. It does not change runtime fallback, native admission, generated media, command registrations, comparison tolerances or measured coverage. The frozen oracle verifier and judging owners remain unchanged. No media corpus or performance round is needed.
