//! User configuration.

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;

use log::*;
use serde::de::{self, Deserializer};
use serde::Deserialize;
use tokio::fs;
use tokio::io;

use crate::syntax::Syntax;

/// Configuration supplied by the user.
#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Config {
    #[serde(default)]
    #[serde(rename = "language-server")]
    pub language_server_config: HashMap<Syntax, LanguageServerConfig>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
pub struct LanguageServerConfig {
    /// The program name and arguments used to launch the language server.
    #[serde(deserialize_with = "validate_command")]
    command: Vec<String>,
}

impl LanguageServerConfig {
    pub fn command(&self) -> (&String, &[String]) {
        self.command
            .split_first()
            .expect("command should not be empty")
    }
}

fn validate_command<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let command = <Vec<String>>::deserialize(deserializer)?;
    if command.is_empty() {
        return Err(de::Error::invalid_length(0, &"at least a program name"));
    }

    Ok(command)
}

impl Config {
    /// Read the configuration from a file path. If no path is supplied, the default configuration
    /// is returned.
    pub async fn read(path: Option<PathBuf>) -> anyhow::Result<Config> {
        // If the file doesn't exist, return the default config.
        let path = match path {
            Some(path) => path,
            None => {
                info!("could not determine config directory");
                return Ok(Config::default());
            }
        };

        info!("reading config from {}", path.display());

        let config = match fs::read(path).await {
            Ok(bytes) => toml::from_slice(&bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                info!("config file not found");
                return Ok(Config::default());
            }
            Err(e) => return Err(e.into()),
        };

        Ok(config)
    }

    /// Returns the path of the config file.
    ///
    /// Respects `XDG_CONFIG_HOME`.
    pub fn config_path() -> Option<PathBuf> {
        let config_dir = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

        Some(config_dir.join("editor/config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ops::Deref;

    use indoc::indoc;
    use maplit::hashmap;
    use tempfile::NamedTempFile;
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    use crate::syntax::Syntax;

    use super::{Config, LanguageServerConfig};

    #[test]
    fn deserialize_empty_config() -> Result<(), Box<dyn Error>> {
        let config = toml::from_str::<Config>("")?;
        assert_eq!(config, Config::default());
        Ok(())
    }

    #[test]
    fn deserialize_language_server() -> Result<(), Box<dyn Error>> {
        let config = toml::from_str::<Config>(indoc!(
            "
            [language-server.rust]
            command = ['rust-analyzer']
            "
        ))?;
        assert_eq!(
            config,
            Config {
                language_server_config: hashmap! {
                    Syntax::Rust => LanguageServerConfig {
                        command: vec![String::from("rust-analyzer")],
                    },
                }
            }
        );
        Ok(())
    }

    #[test]
    fn deserialize_language_server_command_empty() {
        let err = toml::from_str::<Config>(indoc!(
            "
            [language-server.rust]
            command = []
            "
        ))
        .unwrap_err();

        assert!(err.to_string().contains("expected at least a program name"));
    }

    #[tokio::test]
    async fn read_no_config_dir() {
        assert_eq!(Config::read(None).await.unwrap(), Config::default());
    }

    #[tokio::test]
    async fn read_nonexistent_file() {
        let config = Config::read(Some("i-dont-exist.toml".into()))
            .await
            .unwrap();
        assert_eq!(config, Config::default());
    }

    #[tokio::test]
    async fn read_non_toml_file() {
        let (file, path) = NamedTempFile::new().unwrap().into_parts();
        let mut file = File::from_std(file);
        file.write_all(b"I am not TOML").await.unwrap();
        assert!(Config::read(Some(path.deref().into())).await.is_err());
        drop(path);
    }
}
