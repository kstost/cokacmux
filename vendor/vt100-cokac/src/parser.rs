/// A parser for terminal output which produces an in-memory representation of
/// the terminal contents.
pub struct Parser<CB: crate::callbacks::Callbacks = ()> {
    parser: vte::Parser,
    screen: crate::perform::WrappedScreen<CB>,
}

/// Versioned, lossless screen and incremental-decoder checkpoint.
/// Restore only through [`Parser::from_checkpoint`], which validates it.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ParserCheckpoint {
    version: u8,
    parser: vte::Parser,
    screen: crate::Screen,
}

impl std::fmt::Debug for ParserCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParserCheckpoint")
            .field("version", &self.version)
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

impl ParserCheckpoint {
    /// Geometry of the authoritative PTY screen, as (rows, columns).
    pub fn size(&self) -> (u16, u16) {
        self.screen.size()
    }

    /// Approximate heap size, for bounded transport queues.
    pub fn estimated_bytes(&self) -> usize {
        self.screen.checkpoint_bytes()
    }
}

impl Parser {
    /// Restore a checkpoint without replaying already executed control codes.
    pub fn from_checkpoint(checkpoint: ParserCheckpoint) -> Result<Self, &'static str> {
        if checkpoint.version != 1
            || !checkpoint.parser.checkpoint_is_valid()
            || !checkpoint.screen.checkpoint_is_valid()
        {
            return Err("invalid or unsupported terminal checkpoint");
        }
        Ok(Self {
            parser: checkpoint.parser,
            screen: crate::perform::WrappedScreen {
                screen: checkpoint.screen,
                callbacks: (),
                collect_responses: false,
                responses: Vec::new(),
            },
        })
    }

    /// Creates a new terminal parser of the given size and with the given
    /// amount of scrollback.
    #[must_use]
    pub fn new(rows: u16, cols: u16, scrollback_len: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            screen: crate::perform::WrappedScreen::new(rows, cols, scrollback_len),
        }
    }
}

impl<CB: crate::callbacks::Callbacks> Parser<CB> {
    /// Capture both grids, modes, saved cursor and partially parsed input.
    pub fn checkpoint(&self, include_scrollback: bool) -> ParserCheckpoint {
        ParserCheckpoint {
            version: 1,
            parser: self.parser.clone(),
            screen: self.screen.screen.checkpoint(include_scrollback),
        }
    }

    /// Enable device-status and cursor-position replies at parse time.
    pub fn set_collect_responses(&mut self, enabled: bool) {
        self.screen.collect_responses = enabled;
        self.screen.responses.clear();
    }

    /// Drain replies in request order; only PTY owners should write them back.
    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.screen.responses)
    }

    /// Creates a new terminal parser of the given size and with the given
    /// amount of scrollback. Terminal events will be reported via method
    /// calls on the provided [`Callbacks`](crate::callbacks::Callbacks)
    /// implementation.
    pub fn new_with_callbacks(rows: u16, cols: u16, scrollback_len: usize, callbacks: CB) -> Self {
        Self {
            parser: vte::Parser::new(),
            screen: crate::perform::WrappedScreen::new_with_callbacks(
                rows,
                cols,
                scrollback_len,
                callbacks,
            ),
        }
    }

    /// Processes the contents of the given byte string, and updates the
    /// in-memory terminal state.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    /// Returns a reference to a [`Screen`](crate::Screen) object containing
    /// the terminal state.
    #[must_use]
    pub fn screen(&self) -> &crate::Screen {
        &self.screen.screen
    }

    /// Returns a mutable reference to a [`Screen`](crate::Screen) object
    /// containing the terminal state.
    #[must_use]
    pub fn screen_mut(&mut self) -> &mut crate::Screen {
        &mut self.screen.screen
    }

    /// Returns a reference to the [`Callbacks`](crate::callbacks::Callbacks)
    /// state object passed into the constructor.
    pub fn callbacks(&self) -> &CB {
        &self.screen.callbacks
    }

    /// Returns a mutable reference to the
    /// [`Callbacks`](crate::callbacks::Callbacks) state object passed into
    /// the constructor.
    pub fn callbacks_mut(&mut self) -> &mut CB {
        &mut self.screen.callbacks
    }
}

impl Default for Parser {
    /// Returns a parser with dimensions 80x24 and no scrollback.
    fn default() -> Self {
        Self::new(24, 80, 0)
    }
}

impl std::io::Write for Parser {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.process(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
