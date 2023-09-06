//! Helpers for loading and building zspell dictionaries.

pub use spellbook::Dictionary;

use anyhow::Result;

pub fn load_dictionary(_locale: &str) -> Result<Dictionary> {
    let aff = std::fs::read_to_string("/nix/store/sf08lslgs232f4aq0va62rafh3w0w079-hunspell-dict-en-us-wordlist-2018.04.16/share/hunspell/en_US.aff")?;
    let dic = std::fs::read_to_string("/nix/store/sf08lslgs232f4aq0va62rafh3w0w079-hunspell-dict-en-us-wordlist-2018.04.16/share/hunspell/en_US.dic")?;

    let dict = Dictionary::compile(&aff, &dic)?;
    Ok(dict)
}
