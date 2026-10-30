//! The command reference of the documentation site (`docs-site/commands/`,
//! `docs/design/docs-site.md`) shows each command's `--help` as the command
//! prints it, from the snippets in `docs-site/snippets/help/`. This test
//! fails when a snippet and the help differ, or when a command that
//! `coreav --help` lists has no page. `COREAV_BLESS_DOCS=1` rewrites the
//! snippets instead of comparing them.
//!
//! A default build checks every command it has. `thumbnail` and `transcode`
//! exist only in builds with a decoder backend, so their snippets are
//! checked by `cargo test -p coreav-cli --features ffmpeg-fallback --test
//! docs_site` (`ci.yml`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

const DECODER_COMMANDS: [&str; 2] = ["thumbnail", "transcode"];

fn site() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs-site")
}

fn help(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_coreav"))
        .args(args)
        .arg("--help")
        .env_remove("COREAV_STRICT_SAFE")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "coreav {args:?} --help");
    String::from_utf8(out.stdout).unwrap()
}

/// The names under `Commands:` in a help text, without clap's `help`.
fn subcommands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_owned)
        .collect()
}

/// Each snippet's file name and the help it shows: `coreav.mdx` for
/// `coreav --help`, `hls-index.mdx` for `coreav hls index --help`.
/// `coreav ffmpeg --help` is not native (it runs `ffmpeg --help` through
/// the fallback), so it has none.
fn snippets(commands: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if cfg!(not(feature = "thumbnail")) {
        out.push(("coreav".to_owned(), help(&[])));
    }
    for command in commands {
        let decoder = DECODER_COMMANDS.contains(&command.as_str());
        if command == "ffmpeg" || decoder != cfg!(feature = "thumbnail") {
            continue;
        }
        let text = help(&[command]);
        for sub in subcommands(&text) {
            out.push((format!("{command}-{sub}"), help(&[command, &sub])));
        }
        out.push((command.clone(), text));
    }
    out
}

fn snippet(help: &str) -> String {
    format!("```text\n{}\n```\n", help.trim_end())
}

#[test]
fn the_command_reference_matches_the_help() {
    let commands = subcommands(&help(&[]));
    if cfg!(not(feature = "thumbnail")) {
        for command in DECODER_COMMANDS {
            assert!(
                !commands.iter().any(|c| c == command),
                "`coreav {command}` is in a default build, but the documentation says it needs a decoder backend: update its page and the check of its snippet"
            );
        }
    }
    let dir = site().join("snippets/help");
    let bless = std::env::var_os("COREAV_BLESS_DOCS").is_some();
    let expected = snippets(&commands);
    for (name, text) in &expected {
        let path = dir.join(format!("{name}.mdx"));
        let want = snippet(text);
        if bless {
            std::fs::write(&path, want).unwrap();
            continue;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "docs-site/snippets/help/{name}.mdx is not the command's --help: run `COREAV_BLESS_DOCS=1 cargo test -p coreav-cli --test docs_site` (and with `--features ffmpeg-fallback` for thumbnail and transcode), then check that the page beside it still says what the help says"
        );
    }
    if cfg!(not(feature = "thumbnail")) {
        let mut names: Vec<String> = expected.into_iter().map(|(name, _)| name).collect();
        names.extend(DECODER_COMMANDS.map(str::to_owned));
        names.sort();
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter_map(|file| file.strip_suffix(".mdx").map(str::to_owned))
            .collect();
        files.sort();
        assert_eq!(
            files, names,
            "docs-site/snippets/help/ holds a snippet of a command that no longer exists, or lacks one"
        );
    }
}

#[test]
fn every_command_has_a_page() {
    let navigation = std::fs::read_to_string(site().join("docs.json")).unwrap();
    let mut commands = subcommands(&help(&[]));
    if cfg!(not(feature = "thumbnail")) {
        commands.extend(DECODER_COMMANDS.map(str::to_owned));
    }
    for command in commands {
        let page = std::fs::read_to_string(site().join(format!("commands/{command}.mdx")))
            .unwrap_or_default();
        assert!(
            !page.is_empty(),
            "`coreav {command}` has no page: add docs-site/commands/{command}.mdx"
        );
        assert!(
            navigation.contains(&format!("\"commands/{command}\"")),
            "docs-site/docs.json does not list commands/{command}"
        );
        assert!(
            command == "ffmpeg" || page.contains(&format!("from '/snippets/help/{command}.mdx'")),
            "docs-site/commands/{command}.mdx does not show its snippet, /snippets/help/{command}.mdx"
        );
    }
}
