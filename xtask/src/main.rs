mod docgen;
mod helpers;
mod path;
mod theme_check;

use std::{env, error::Error};

type DynError = Box<dyn Error>;

pub mod tasks {
    use crate::docgen::{lang_features, typable_commands, write};
    use crate::docgen::{LANG_SUPPORT_MD_OUTPUT, TYPABLE_COMMANDS_MD_OUTPUT};
    use crate::theme_check::theme_check;
    use crate::DynError;

    pub fn docgen() -> Result<(), DynError> {
        write(TYPABLE_COMMANDS_MD_OUTPUT, &typable_commands()?);
        write(LANG_SUPPORT_MD_OUTPUT, &lang_features()?);
        Ok(())
    }

    pub fn themecheck() -> Result<(), DynError> {
        theme_check()
    }

    pub fn print_help() {
        println!(
            "
Usage: Run with `cargo xtask <task>`, eg. `cargo xtask docgen`.

    Tasks:
        docgen: Generate files to be included in the mdbook output.
"
        );
    }
}

fn main() -> Result<(), DynError> {
    let task = env::args().nth(1);
    match task {
        None => tasks::print_help(),
        Some(t) => match t.as_str() {
            "docgen" => tasks::docgen()?,
            "theme-check" => tasks::themecheck()?,
            invalid => return Err(format!("Invalid task name: {}", invalid).into()),
        },
    };
    Ok(())
}
