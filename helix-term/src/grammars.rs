use anyhow::{anyhow, Context, Result};
use std::fs;
use std::time::SystemTime;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

use helix_core::syntax::{GrammarConfiguration, DYLIB_EXTENSION};

const BUILD_TARGET: &str = env!("BUILD_TARGET");

pub fn build_grammars() {
    let builtin_err_msg = "Could not parse built-in languages.toml, something must be very wrong";

    let config: helix_core::syntax::Configuration =
        toml::from_slice(include_bytes!("../../languages.toml")).expect(builtin_err_msg);

    for grammar in config.grammar {
        let grammar_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../runtime/grammars/sources")
            .join(grammar.grammar_id.clone());

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
