use std::ffi::OsString;
use std::io::Write;
use std::process::ExitCode;

use crate::strict_safe::{FLAG, StrictSafe};

pub(crate) fn worker_main(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    let mut args = args.into_iter().peekable();
    let strict_safe = StrictSafe::new(args.next_if(|arg| arg == FLAG).is_some());
    if args.next().is_some() {
        let _ = writeln!(std::io::stderr(), "invalid private AV1 worker arguments");
        return ExitCode::from(2);
    }
    if let Err(message) = strict_safe.sandbox() {
        let _ = writeln!(std::io::stderr(), "{message}");
        return ExitCode::from(4);
    }
    if !cfg!(feature = "svt-av1-native") || !coreav_transcode::Av1Transcode::compiled() {
        let _ = writeln!(
            std::io::stderr(),
            "no AV1 encoder: this build has no native SVT-AV1 backend for this target"
        );
        return ExitCode::from(4);
    }
    #[cfg(feature = "svt-av1-native")]
    {
        if coreav_backend_svt_av1::worker::run().is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
    #[cfg(not(feature = "svt-av1-native"))]
    ExitCode::from(4)
}
