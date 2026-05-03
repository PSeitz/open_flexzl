use thiserror::Error as ThisError;

/// Error type for the future `open_flexzl` implementation.
///
/// The implementation has intentionally been reset to planning stubs. Expand
/// this enum when implementing the approved format in `open_flexzl/plan.md`.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("open_flexzl implementation is not available yet; see open_flexzl/plan.md")]
    NotImplemented,
}
