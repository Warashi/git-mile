pub mod clock;
pub mod error;

pub use clock::{LamportClock, LamportTimestamp, ReplicaId};
pub use error::{Error, Result};

pub const APP_NAME: &str = "git-mile";

pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_application_identity() {
        assert_eq!(APP_NAME, "git-mile");
        assert_eq!(app_version(), env!("CARGO_PKG_VERSION"));
    }
}
