//! Context-wrapper conversion helpers (re-exports for clarity).

use crate::error::Result;
use crate::universal::{Provider, UniversalSession};
use crate::{SessionSource, SessionTarget};

pub fn convert(
    from: Provider,
    to: Provider,
    src: &SessionSource,
    dst: &SessionTarget,
) -> Result<UniversalSession> {
    crate::convert(from, to, src, dst)
}
