use std::io;
use std::path::PathBuf;

use crate::geocode::Place;

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

/// Invalid map data comes from the tile server, so it is a provider error.
impl From<tmaps_core::Error> for Error {
    fn from(error: tmaps_core::Error) -> Self {
        match error {
            tmaps_core::Error::InvalidInput(message) => Self::InvalidInput(message),
            tmaps_core::Error::InvalidData(message) => Self::Provider(message),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    App(#[from] Error),
    #[error(
        "{input:?} matches more than one place; use a more specific query, \"lat,lon\", or @place"
    )]
    Ambiguous {
        input: String,
        candidates: Vec<Place>,
    },
    #[error("{0}")]
    Usage(String),
    #[error(transparent)]
    Map(#[from] crate::tui::Error),
    #[error("cannot write output: {0}")]
    Output(#[from] io::Error),
}

impl CliError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::App(error) | Self::Map(crate::tui::Error::App(error)) => error.kind(),
            Self::Ambiguous { .. } => "ambiguous",
            Self::Usage(_) => "usage",
            Self::Map(_) => "map",
            Self::Output(_) => "output",
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::App(error) | Self::Map(crate::tui::Error::App(error)) => error.exit_code(),
            Self::Ambiguous { .. } | Self::Usage(_) => 2,
            Self::Map(_) | Self::Output(_) => 1,
        }
    }

    pub fn candidates(&self) -> Option<&[Place]> {
        match self {
            Self::Ambiguous { candidates, .. } => Some(candidates),
            _ => None,
        }
    }
}

impl From<tmaps_core::Error> for CliError {
    fn from(error: tmaps_core::Error) -> Self {
        Self::App(error.into())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Output(error.into())
    }
}
