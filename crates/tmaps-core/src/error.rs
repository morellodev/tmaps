use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("no route found: {0}")]
    NoRoute(String),
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("config error in {}: {message}", path.display())]
    Config { path: PathBuf, message: String },
    #[error("cannot find the home directory")]
    NoHomeDir,
}
