pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// A vector tile or a `TileJSON` document that cannot be read.
    #[error("invalid data: {0}")]
    InvalidData(String),
}
