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
    /// Home Assistant refused the token (401).
    Unauthorized,
    /// Home Assistant refused the request (403).
    Forbidden,
    /// The entity does not exist (404).
    NotFound,
    /// 401 on the template endpoint: it needs an administrator account.
    AdminNeeded,
    /// The import file does not exist.
    ImportMissing,
    /// The import file exists but could not be read.
    ImportRead,
    /// The import file holds no token.
    ImportEmpty,
    /// The imported token holds characters a header cannot carry.
    ImportInvalid,
    /// The imported token is longer than the limit.
    ImportTooLong,
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
