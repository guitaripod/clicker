//! Error type carrying the process exit code each failure maps to.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Protocol(String),

    #[error("{0}")]
    Authentication(String),

    #[error(
        "the Apple TV is limiting pairing attempts; wait {} seconds before trying again",
        seconds.unwrap_or(60)
    )]
    PairingBackoff { seconds: Option<u64> },

    #[error("timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("no Apple TV matching \"{0}\"")]
    DeviceNotFound(String),

    #[error("{0}")]
    NotPaired(String),

    #[error("{0}")]
    Config(String),

    #[error("{0}")]
    Discovery(String),

    #[error("{0}")]
    Usage(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Opack(#[from] crate::opack::Error),

    /// Already reported to the user by the command itself; only the exit code
    /// still matters.
    #[error("see the message above")]
    Reported { exit_code: i32 },
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => 2,
            Error::DeviceNotFound(_) => 3,
            Error::NotPaired(_) => 4,
            Error::Authentication(_) | Error::PairingBackoff { .. } => 5,
            Error::Timeout(_) => 6,
            Error::Protocol(_) | Error::Opack(_) => 7,
            Error::Config(_) | Error::Discovery(_) => 8,
            Error::Reported { exit_code } => *exit_code,
            Error::Io(_) => 1,
        }
    }

    /// Stable machine-readable code for `--json` output.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Usage(_) => "usage",
            Error::DeviceNotFound(_) => "device_not_found",
            Error::NotPaired(_) => "not_paired",
            Error::Authentication(_) => "authentication_failed",
            Error::PairingBackoff { .. } => "pairing_backoff",
            Error::Timeout(_) => "timeout",
            Error::Protocol(_) => "protocol_error",
            Error::Opack(_) => "opack_error",
            Error::Config(_) => "config_error",
            Error::Discovery(_) => "discovery_error",
            Error::Io(_) => "io_error",
            Error::Reported { .. } => "reported",
        }
    }

    pub fn hint(&self) -> Option<String> {
        match self {
            Error::NotPaired(_) => {
                Some("run `clicker setup` to find your Apple TV and pair with it".into())
            }
            Error::DeviceNotFound(_) => {
                Some("run `clicker scan` to list devices on this network".into())
            }
            Error::Authentication(_) => {
                Some("the Apple TV no longer trusts these credentials; pair again".into())
            }
            Error::PairingBackoff { .. } => {
                Some("each failed attempt makes the wait longer, so read the code carefully".into())
            }
            Error::Timeout(_) => Some("the device may be asleep; try `clicker power on`".into()),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
