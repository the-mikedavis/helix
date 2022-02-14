use anyhow::{anyhow, Context, Result};
use std::fs;
use std::time::SystemTime;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::channel,
};

use helix_core::syntax::{GrammarConfiguration, GrammarSource, DYLIB_EXTENSION};

const BUILD_TARGET: &str = env!("BUILD_TARGET");
const REMOTE_NAME: &str = "helix-origin";

pub fn fetch_grammars() {
    run_parallel(get_grammar_configs(), fetch_grammar);
}

pub fn build_grammars() {
    run_parallel(get_grammar_configs(), build_grammar);
}

fn run_parallel<F>(grammars: Vec<GrammarConfiguration>, job: F)
where
    F: Fn(GrammarConfiguration) + std::marker::Send + 'static + Copy,
{
    let mut n_jobs = 0;
    let pool = threadpool::Builder::new().build();
    let (tx, rx) = channel();

    for grammar in grammars {
        let tx = tx.clone();
        n_jobs += 1;

        pool.execute(move || {
            job(grammar);

            // report progress
            tx.send(1).unwrap();
        });
    }
    pool.join();

    assert_eq!(rx.try_iter().sum::<usize>(), n_jobs);
}

pub fn fetch_grammar(grammar: GrammarConfiguration) {
    if let GrammarSource::Git { remote, revision } = grammar.source {
        let grammar_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../runtime/grammars/sources")
            .join(grammar.grammar_id.clone());

        fs::create_dir_all(grammar_dir.clone()).expect("Could not create grammar directory");

        // create the grammar dir contains a git directory
        if !grammar_dir.join(".git").is_dir() {
            Command::new("git")
                .args(["init"])
                .current_dir(grammar_dir.clone())
                .output()
                .expect("Could not execute 'git'");
        }

        // ensure the remote matches the configured remote
        if get_repository_info(&grammar_dir, vec!["remote", "get-url", REMOTE_NAME])
            != Some(remote.clone())
        {
            set_remote(&grammar_dir, &remote);
        }

        // ensure the revision matches the configured revision
        if get_repository_info(&grammar_dir, vec!["rev-parse", "HEAD"]) != Some(revision.clone()) {
            // Fetch the exact revision from the remote.
            // Supported by server-side git since v2.5.0 (July 2015),
            // enabled by default on major git hosts.
            Command::new("git")
                .args(["fetch", REMOTE_NAME, &revision])
                .current_dir(grammar_dir.clone())
                .output()
                .expect("Failed to execute 'git'");

            Command::new("git")
                .args(["checkout", &revision])
                .current_dir(grammar_dir)
                .output()
                .expect("Failed to execute 'git'");

            println!(
                "Grammar '{}' checked out at '{}'.",
                grammar.grammar_id, revision
            );
        } else {
            println!("Grammar '{}' is already up to date.", grammar.grammar_id);
        }
    };
}

// Sets the remote for a repository to the given URL, creating the remote if
// it does not yet exist.
fn set_remote(repository: &Path, remote_url: &String) {
    if !Command::new("git")
        .args(["remote", "set-url", REMOTE_NAME, remote_url])
        .current_dir(repository.clone())
        .output()
        .expect("Failed to execute 'git'")
        .status
        .success()
    {
        if !Command::new("git")
            .args(["remote", "add", REMOTE_NAME, remote_url])
            .current_dir(repository.clone())
            .output()
            .expect("Failed to execute 'git'")
            .status
            .success()
        {
            eprintln!("Failed to set remote '{}'", *remote_url);
        }
    }
}

fn get_repository_info(repository: &Path, args: Vec<&str>) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository.clone())
        .output()
        .expect("Failed to execute 'git'");
    if output.status.success() {
        let mut remote = String::from_utf8_lossy(output.stdout.as_slice()).into_owned();
        // remove trailing newline
        remote.pop();
        Some(remote)
    } else {
        None
    }
}

fn build_grammar(grammar: GrammarConfiguration) {
    let grammar_dir = if let GrammarSource::Local { ref path } = grammar.source {
        PathBuf::from(path)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../runtime/grammars/sources")
            .join(grammar.grammar_id.clone())
    };

    if grammar_dir.read_dir().is_err() {
        eprintln!(
            "The directory {:?} is empty, you probably need to use 'hx --fetch-grammars'?",
            grammar_dir
        );
        std::process::exit(1);
    }

    let path = match grammar.path {
        Some(ref subpath) => grammar_dir.join(subpath),
        None => grammar_dir,
    }
    .join("src");

    build_library(&path, grammar).unwrap();
}

fn get_grammar_configs() -> Vec<GrammarConfiguration> {
    let builtin_err_msg = "Could not parse built-in languages.toml, something must be very wrong";

    // TODO prefer user config and default to the built-in.
    let config: helix_core::syntax::Configuration =
        toml::from_slice(include_bytes!("../../languages.toml")).expect(builtin_err_msg);

    config.grammar
}

fn build_library(src_path: &Path, grammar: GrammarConfiguration) -> Result<()> {
    let header_path = src_path;
    // let grammar_path = src_path.join("grammar.json");
    let parser_path = src_path.join("parser.c");
    let mut scanner_path = src_path.join("scanner.c");

    let scanner_path = if scanner_path.exists() {
        Some(scanner_path)
    } else {
        scanner_path.set_extension("cc");
        if scanner_path.exists() {
            Some(scanner_path)
        } else {
            None
        }
    };
    let parser_lib_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../runtime/grammars");
    let mut library_path = parser_lib_path.join(grammar.grammar_id.clone());
    library_path.set_extension(DYLIB_EXTENSION);

    let recompile = needs_recompile(&library_path, &parser_path, &scanner_path)
        .with_context(|| "Failed to compare source and binary timestamps")?;

    if !recompile {
        println!("Grammar '{}' is already built.", grammar.grammar_id);
        return Ok(());
    }

    println!("Building grammar '{}'", grammar.grammar_id);

    let mut config = cc::Build::new();
    config
        .cpp(true)
        .opt_level(2)
        .cargo_metadata(false)
        .host(BUILD_TARGET)
        .target(BUILD_TARGET);
    let compiler = config.get_compiler();
    let mut command = Command::new(compiler.path());
    command.current_dir(src_path);
    for (key, value) in compiler.env() {
        command.env(key, value);
    }

    if cfg!(windows) {
        command
            .args(&["/nologo", "/LD", "/I"])
            .arg(header_path)
            .arg("/Od")
            .arg("/utf-8");
        if let Some(scanner_path) = scanner_path.as_ref() {
            command.arg(scanner_path);
        }

        command
            .arg(parser_path)
            .arg("/link")
            .arg(format!("/out:{}", library_path.to_str().unwrap()));
    } else {
        command
            .arg("-shared")
            .arg("-fPIC")
            .arg("-fno-exceptions")
            .arg("-g")
            .arg("-I")
            .arg(header_path)
            .arg("-o")
            .arg(&library_path)
            .arg("-O2");
        if let Some(scanner_path) = scanner_path.as_ref() {
            if scanner_path.extension() == Some("c".as_ref()) {
                command.arg("-xc").arg("-std=c99").arg(scanner_path);
            } else {
                command.arg(scanner_path);
            }
        }
        command.arg("-xc").arg(parser_path);
        if cfg!(all(unix, not(target_os = "macos"))) {
            command.arg("-Wl,-z,relro,-z,now");
        }
    }

    let output = command
        .output()
        .with_context(|| "Failed to execute C compiler")?;
    if !output.status.success() {
        return Err(anyhow!(
            "Parser compilation failed.\nStdout: {}\nStderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(())
}

fn needs_recompile(
    lib_path: &Path,
    parser_c_path: &Path,
    scanner_path: &Option<PathBuf>,
) -> Result<bool> {
    if !lib_path.exists() {
        return Ok(true);
    }
    let lib_mtime = mtime(lib_path)?;
    if mtime(parser_c_path)? > lib_mtime {
        return Ok(true);
    }
    if let Some(scanner_path) = scanner_path {
        if mtime(scanner_path)? > lib_mtime {
            return Ok(true);
        }
    }
    Ok(false)
}

fn mtime(path: &Path) -> Result<SystemTime> {
    Ok(fs::metadata(path)?.modified()?)
}
