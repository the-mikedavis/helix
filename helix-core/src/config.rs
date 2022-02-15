use crate::merge_toml_values;

/// Default bultin-in languages.toml.
pub fn default_lang_config() -> toml::Value {
    toml::from_slice(include_bytes!("../../languages.toml"))
        .expect("Could not parse bultin-in languages.toml to valid toml")
}

/// User-only configured languages.toml, or None if the user does not define one.
pub fn user_lang_config() -> Option<Result<toml::Value, toml::de::Error>> {
    match std::fs::read(crate::config_dir().join("languages.toml")) {
        Ok(raw) => Some(toml::from_slice(&raw)),
        Err(_) => None,
    }
}

/// User configured languages.toml file, merged with the default config.
pub fn merged_lang_config() -> Result<toml::Value, toml::de::Error> {
    let def_lang_conf = default_lang_config();
    let merged_lang_conf = match user_lang_config() {
        Some(toml) => merge_toml_values(def_lang_conf, toml?),
        None => def_lang_conf,
    };

    Ok(merged_lang_conf)
}

/// Syntax configuration loader based on built-in languages.toml.
pub fn default_syntax_loader() -> crate::syntax::Configuration {
    default_lang_config()
        .try_into()
        .expect("Could not serialize built-in languages.toml")
}

/// Syntax configuration loader based only on user configured languages.toml.
pub fn user_syntax_loader() -> Option<Result<crate::syntax::Configuration, toml::de::Error>> {
    user_lang_config().map(|config| config?.try_into())
}

/// Syntax configuration loader based on user configured languages.toml merged
/// with the default languages.toml
pub fn merged_syntax_loader() -> Result<crate::syntax::Configuration, toml::de::Error> {
    merged_lang_config()?.try_into()
}
