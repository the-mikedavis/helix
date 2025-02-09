use std::path::PathBuf;

use anyhow::Result;

pub use skidder::{Config, Repo};

#[derive(Debug)]
pub struct Loader {
    config: Config,
}

impl Default for Loader {
    fn default() -> Self {
        let repos = vec![Repo::Git {
            name: "upstream".to_string(),
            remote: "https://github.com/helix-editor/tree-sitter-grammars".to_string(),
            branch: "main".to_string(),
        }];

        Self {
            config: Config {
                repos,
                index: crate::language_support_dir(),
                verbose: true,
            },
        }
    }
}

impl Loader {
    pub fn new(sources: &[Repo]) -> Self {
        let mut loader = Self::default();
        // TODO: ensure the default one is last?
        loader.config.repos.extend(sources.iter().cloned());

        if let Some(default_repo) = option_env!("HELIX_DEFAULT_LANGUAGE_SUPPORT_REPO") {
            loader.config.repos.push(skidder::Repo::Local {
                path: default_repo.into(),
            });
        }

        // TODO: be able to compile in a repo with lower precedence than the default?

        loader
    }

    pub fn get_compiled_parser_path(&self, name: &str) -> Option<(String, PathBuf)> {
        self.config.compiled_parser_path(name)
    }

    pub fn grammar_dir(&self, grammar: &str) -> Option<PathBuf> {
        self.config.grammar_dir(grammar)
    }

    pub fn repository_dirs(&self) -> impl Iterator<Item = (&Repo, PathBuf)> + '_ {
        self.config
            .repos
            .iter()
            .map(|repo| (repo, repo.dir(&self.config)))
    }
}

pub fn update_grammars(config: &Loader) -> Result<()> {
    println!("Fetching language support...");
    skidder::fetch(&config.config, true)?;
    println!("Building tree-sitter parsers...");
    skidder::build_all_grammars(&config.config, false, None)?;
    println!("Language support updated successfully");
    Ok(())
}
