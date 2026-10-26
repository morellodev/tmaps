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

impl Error {
    /// Stable `snake_case` name of the error kind, for machine-readable output.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::NoRoute(_) => "no_route",
            Self::Network(_) => "network",
            Self::Provider(_) => "provider",
            Self::Config { .. } | Self::NoHomeDir => "config",
        }
    }

    /// Process exit code: 1 config, 2 invalid input, 3 no route, 4 network or provider.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Config { .. } | Self::NoHomeDir => 1,
            Self::InvalidInput(_) => 2,
            Self::NoRoute(_) => 3,
            Self::Network(_) | Self::Provider(_) => 4,
        }
    }
}
