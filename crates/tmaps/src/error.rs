use std::io;

use tmaps_core::geocode::Place;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Core(#[from] tmaps_core::Error),
    #[error(
        "{input:?} matches more than one place; use a more specific query, \"lat,lon\", or @place"
    )]
    Ambiguous {
        input: String,
        candidates: Vec<Place>,
    },
    #[error("{0}")]
    Usage(String),
    #[error("cannot write output: {0}")]
    Output(#[from] io::Error),
}

impl CliError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Core(error) => error.kind(),
            Self::Ambiguous { .. } => "ambiguous",
            Self::Usage(_) => "usage",
            Self::Output(_) => "output",
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Core(error) => error.exit_code(),
            Self::Ambiguous { .. } | Self::Usage(_) => 2,
            Self::Output(_) => 1,
        }
    }

    pub fn candidates(&self) -> Option<&[Place]> {
        match self {
            Self::Ambiguous { candidates, .. } => Some(candidates),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::Output(error.into())
    }
}
