//! Failures shown to the user, from the network to the entity states.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The configuration is missing or cannot be used.
    Configuration,
    /// No token file, or an empty one.
    MissingToken,
    /// The network device could not be opened, or the connection failed.
    Network,
    /// The server did not answer in time.
    Timeout,
    /// Home Assistant refused the token (401 or 403).
    Unauthorized,
    /// The refresh token was revoked (400 on a token refresh): the device
    /// is signed out.
    Revoked,
    /// The entity does not exist (404).
    NotFound,
    /// Any other unexpected HTTP status.
    Http(u16),
    /// The response is not the JSON that was expected.
    InvalidResponse,
    /// A string or a response part is larger than the application keeps.
    TooLarge,
    /// The configuration or token file could not be read or written.
    Storage,
}

pub type Result<T> = core::result::Result<T, Error>;
