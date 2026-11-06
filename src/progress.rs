use crate::{context::Context, sync::unpoisoned};
use crossterm::{
    QueueableCommand, cursor,
    terminal::{self, ClearType},
};
use std::{
    io::{self, Write},
    process,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// How often the indicator redraws the entry count.
const REFRESH: Duration = Duration::from_millis(50);

/// Exit status after an interrupt: 128 + `SIGINT`, as a shell reports it.
const INTERRUPTED: i32 = 130;

/// Responsible for displying the progress indicator. This struct will be owned by a separate
/// thread that is responsible for displaying the progress text whereas the [`IndicatorHandle`]
/// is how the outside world will interact with it.
pub struct Indicator {
    count: u64,
    /// Whether the cursor is hidden and the indicator line drawn, so that the terminal has to be
    /// restored. Nothing is written to the terminal before the walk has been running for one
    /// [`REFRESH`] tick. The interrupt handler shares it: whoever holds the lock owns the
    /// terminal, so the handler can restore it and exit without racing a redraw.
    hidden: Arc<Mutex<bool>>,
}

/// How the outside world stops the [`Indicator`] thread and feeds it the number of entries
/// indexed so far.
pub struct IndicatorHandle {
    thread: Option<JoinHandle<Result<(), Error>>>,
    stop: SyncSender<()>,
    /// Entries indexed so far; the walk increments it and the [`Indicator`] polls it.
    indexed: Arc<AtomicU64>,
}

/// Errors associated with [`crossterm`];
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] io::Error),

    #[error("Failed to set interrupt handler: {0}")]
    Interrupt(#[from] ctrlc::Error),

    #[error("The progress indicator thread panicked")]
    Panicked,
}

impl IndicatorHandle {
    /// Counter of indexed entries, for the walk to increment.
    pub fn indexed(&self) -> &AtomicU64 {
        &self.indexed
    }

    /// Stops the [`Indicator`] and waits until the terminal is restored. Output may be written to
    /// stdout only afterwards.
    pub fn finish(mut self) -> Result<(), Error> {
        self.stop()
    }

    fn stop(&mut self) -> Result<(), Error> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        // Fails only if the thread has already returned with an error, which `join` reports.
        let _ = self.stop.send(());
        thread.join().map_err(|_e| Error::Panicked)?
    }
}

impl Drop for IndicatorHandle {
    /// Makes sure the [`Indicator`] thread is torn down, and thus the cursor restored, even when
    /// the owner unwinds from a panic or returns early.
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

impl Drop for Indicator {
    /// Once the cursor is hidden, always restore the terminal, including when the owning thread
    /// fails with an error or panics.
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

impl Indicator {
    /// Initializes an [`Indicator`] returning an [`IndicatorHandle`] if a progress indicator is
    /// enabled via [`Context`]. The interrupt handler is registered, and the terminal touched,
    /// only once the walk outlasts one [`REFRESH`] tick; failing to register the handler is then
    /// reported by [`IndicatorHandle::finish`].
    pub fn maybe_init(ctx: &Context) -> Option<IndicatorHandle> {
        if !ctx.stdout_is_tty || ctx.no_progress {
            return None;
        }

        Some(Self::measure())
    }

    /// Spawns the thread that owns the [`Indicator`] and redraws the count every [`REFRESH`]
    /// until it is told to stop.
    fn measure() -> IndicatorHandle {
        let (stop, rx) = mpsc::sync_channel(1);
        let indexed = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&indexed);

        let thread = thread::spawn(move || {
            let mut indicator = Self {
                count: 0,
                hidden: Arc::new(Mutex::new(false)),
            };

            // The handle sends before it drops the sender, so a disconnect is only a fallback.
            while rx.recv_timeout(REFRESH) == Err(RecvTimeoutError::Timeout) {
                let count = counter.load(Ordering::Relaxed);
                if count != indicator.count {
                    indicator.index(count)?;
                }
            }

            indicator.finish()
        });

        IndicatorHandle {
            thread: Some(thread),
            stop,
            indexed,
        }
    }

    /// Draws `count`. The first draw registers the interrupt handler, then saves the cursor
    /// position and hides the cursor. The handler comes first so that an interrupt can never
    /// leave the cursor hidden. A `SIGINT` before this point meets the default handler, and the
    /// terminal is untouched.
    // The lock is held across the write so that the interrupt handler never meets a half-drawn
    // line.
    #[allow(clippy::significant_drop_tightening)]
    fn index(&mut self, count: u64) -> Result<(), Error> {
        let mut hidden = unpoisoned(self.hidden.lock());
        let mut stdout = io::stdout().lock();

        if !*hidden {
            let shared = Arc::clone(&self.hidden);
            ctrlc::set_handler(move || interrupt(&shared))?;

            *hidden = true;
            stdout.queue(cursor::SavePosition)?.queue(cursor::Hide)?;
        }

        self.count = count;
        write!(stdout, "Indexing {count} files...")?;
        stdout.queue(cursor::RestorePosition)?.flush()?;
        Ok(())
    }

    /// Clears the indicator and shows the cursor again, unless nothing was ever drawn.
    // The lock is held until the terminal is restored; released earlier, an interrupt in between
    // would exit with the cursor hidden.
    #[allow(clippy::significant_drop_tightening)]
    fn finish(&self) -> Result<(), Error> {
        let mut hidden = unpoisoned(self.hidden.lock());
        if *hidden {
            *hidden = false;
            restore(&mut io::stdout().lock())?;
        }
        Ok(())
    }
}

/// The interrupt handler: restores the terminal if the indicator is drawn, then exits. Holding the
/// lock until the exit keeps the indicator from drawing again. Once the indicator has finished,
/// `main` may hold the stdout lock while it renders, but then the terminal needs no restoring and
/// stdout is not touched here (the exit itself only tries the lock).
#[allow(clippy::significant_drop_tightening)]
fn interrupt(hidden: &Mutex<bool>) {
    let hidden = unpoisoned(hidden.lock());
    if *hidden {
        let _ = restore(&mut io::stdout().lock());
    }
    process::exit(INTERRUPTED);
}

/// Clears the indicator line and shows the cursor at the position saved before the first draw.
fn restore(stdout: &mut impl Write) -> io::Result<()> {
    stdout
        .queue(terminal::Clear(ClearType::CurrentLine))?
        .queue(cursor::RestorePosition)?
        .queue(cursor::Show)?
        .flush()
}
