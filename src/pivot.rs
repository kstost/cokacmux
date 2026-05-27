//! X → U → Y composition helpers (re-exports for clarity).

use crate::error::Result;
use crate::universal::{Provider, UniversalSession};
use crate::{read_session, write_session, SessionSource, SessionTarget};

pub fn convert(
    from: Provider,
    to: Provider,
    src: &SessionSource,
    dst: &SessionTarget,
) -> Result<UniversalSession> {
    let session = read_session(from, src)?;
    write_session(to, &session, dst)?;
    Ok(session)
}
