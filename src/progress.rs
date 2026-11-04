use crate::{context::Context, tty};
use crossterm::{
    ExecutableCommand, cursor,
    terminal::{self, ClearType},
};
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, SendError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// How often the indicator redraws the entry count.
const REFRESH: Duration = Duration::from_millis(50);

/// Responsible for displying the progress indicator. This struct will be owned by a separate
/// thread that is responsible for displaying the progress text whereas the [`IndicatorHandle`]
/// is how the outside world will interact with it.
pub struct Indicator<'a> {
    count: u64,
    stdout: io::StdoutLock<'a>,
    /// Whether the cursor was hidden, so that it has to be restored. Nothing is written to the
    /// terminal before the walk has been running for one [`REFRESH`] tick.
    hidden: bool,
}

/// This struct is how the outside world will inform the [`Indicator`] about the progress of the
/// program. The `join_handle` returns the handle to the thread that owns the [`Indicator`] and the
/// `mailbox` is the [`SyncSender`] channel that allows [`Message`]s to be sent to [`Indicator`].
pub struct IndicatorHandle {
    pub join_handle: Option<JoinHandle<Result<(), Error>>>,
    mailbox: SyncSender<Message>,
    /// Entries indexed so far; the walk increments it and the [`Indicator`] polls it.
    indexed: Arc<AtomicU64>,
}

/// The different messages that could be sent to the thread that owns the [`Indicator`].
#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    /// Message that indicates that the output is ready to be flushed and that we should cleanup
    /// the [`Indicator`] as well as the screen.
    RenderReady,

    /// Tear down the progress indicator for whatever reason.
    Finish,
}

/// Errors associated with [`crossterm`];
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum Error {
    #[error("#{0}")]
    Io(#[from] io::Error),

    #[error("#{0}")]
    Send(#[from] SendError<Message>),

    #[error("Failed to set interrupt handler: {0}")]
    Interrupt(#[from] ctrlc::Error),

    #[error("The progress indicator thread panicked")]
    Panicked,
}

impl Default for Indicator<'_> {
    /// Default constructor for [`Indicator`].
    fn default() -> Self {
        Self {
            count: u64::default(),
            stdout: io::stdout().lock(),
            hidden: false,
        }
    }
}

impl IndicatorHandle {
    /// The constructor for an [`IndicatorHandle`].
    pub const fn new(
        join_handle: Option<JoinHandle<Result<(), Error>>>,
        mailbox: SyncSender<Message>,
        indexed: Arc<AtomicU64>,
    ) -> Self {
        Self {
            join_handle,
            mailbox,
            indexed,
        }
    }

    /// Counter of indexed entries, for the walk to increment.
    pub fn indexed(&self) -> &AtomicU64 {
        &self.indexed
    }

    /// Getter for a cloned `mailbox` wherewith to send [`Message`]s to the [`Indicator`].
    pub fn mailbox(&self) -> SyncSender<Message> {
        self.mailbox.clone()
    }

    /// Waits for the [`Indicator`] thread to finish, if it hasn't been joined already.
    pub fn join(&mut self) -> Result<(), Error> {
        self.join_handle
            .take()
            .map_or(Ok(()), |h| h.join().map_err(|_e| Error::Panicked)?)
    }

    /// Attempts to terminate the [`Indicator`] with cleanup.
    pub fn try_terminate(this: Option<Self>) -> Result<(), Error> {
        if let Some(mut handle) = this {
            // This is allowed to fail silently. If user administers interrupt then the `Indicator`
            // will be dropped along with the receiving end of the `mailbox`.
            //
            // If user does not administer interrupt but file-system traversal fails for whatever
            // reason then this will proceed as normal.
            let _ = handle.mailbox().send(Message::Finish);

            handle.join()?;
        }

        Ok(())
    }
}

impl Drop for IndicatorHandle {
    /// Makes sure the [`Indicator`] thread is torn down, and thus the cursor restored, even when
    /// the owner unwinds from a panic or returns early.
    fn drop(&mut self) {
        if self.join_handle.is_some() {
            let _ = self.mailbox.send(Message::Finish);
            let _ = self.join();
        }
    }
}

impl Drop for Indicator<'_> {
    /// Once the cursor is hidden, always show it again, including when the owning thread fails
    /// with an error or panics.
    fn drop(&mut self) {
        if self.hidden {
            let _ = self.stdout.execute(cursor::Show);
        }
    }
}

impl Indicator<'_> {
    /// Initializes an [`Indicator`] returning an [`IndicatorHandle`] if a progress indicator is
    /// enabled via [`Context`]. The interrupt handler is registered, and the terminal touched,
    /// only once the walk outlasts one [`REFRESH`] tick; failing to register the handler is then
    /// reported when the thread is joined.
    pub fn maybe_init(ctx: &Context) -> Option<IndicatorHandle> {
        if !ctx.stdout_is_tty || ctx.no_progress {
            return None;
        }

        Some(Indicator::measure())
    }

    /// Initializes a worker thread that owns [`Indicator`] that awaits on [`Message`]s to traverse
    /// through its internal states. An [`IndicatorHandle`] is returned as a mechanism to allow the
    /// outside world to send messages to the worker thread and ultimately to the [`Indicator`].
    pub fn measure() -> IndicatorHandle {
        let (tx, rx) = mpsc::sync_channel(1024);
        let indexed = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&indexed);

        let interrupt_mailbox = tx.clone();

        let join_handle = thread::spawn(move || {
            let mut indicator = Self::default();

            loop {
                match rx.recv_timeout(REFRESH) {
                    Ok(Message::RenderReady | Message::Finish) => {
                        indicator.finish()?;
                        return Ok(());
                    },
                    Err(RecvTimeoutError::Timeout) => {
                        let count = counter.load(Ordering::Relaxed);
                        if count != indicator.count {
                            if !indicator.hidden {
                                indicator.start(&interrupt_mailbox)?;
                            }
                            indicator.index(count)?;
                            indicator.stdout.execute(cursor::RestorePosition)?;
                        }
                    },
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }
        });

        IndicatorHandle::new(Some(join_handle), tx, indexed)
    }

    /// Registers the interrupt handler, then saves the cursor position and hides the cursor. The
    /// handler comes first so that an interrupt can never leave the cursor hidden. A `SIGINT`
    /// before this point meets the default handler, and the terminal is untouched.
    fn start(&mut self, mailbox: &SyncSender<Message>) -> Result<(), Error> {
        let mailbox = mailbox.clone();

        ctrlc::set_handler(move || {
            let _ = mailbox.try_send(Message::Finish);
            let _ = tty::restore();
        })?;

        self.hidden = true;
        self.stdout.execute(cursor::SavePosition)?;
        self.stdout.execute(cursor::Hide)?;
        Ok(())
    }

    /// Clears the indicator and shows the cursor again, unless nothing was ever drawn.
    fn finish(&mut self) -> Result<(), Error> {
        if !self.hidden {
            return Ok(());
        }

        let stdout = &mut self.stdout;
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        stdout.execute(cursor::RestorePosition)?;
        stdout.execute(cursor::Show)?;
        Ok(())
    }

    /// The user-facing output while indexing.
    #[inline]
    fn index(&mut self, count: u64) -> Result<(), Error> {
        self.count = count;
        write!(self.stdout, "Indexing {} files...", self.count)?;
        Ok(())
    }
}
