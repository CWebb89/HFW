//! A job that runs on its own thread, so a caller can draw its progress and
//! cancel it.
//!
//! Everything else in this library answers within the call. Planning and writing
//! a plate takes minutes, and a settings panel that freezes for minutes reads as
//! a crash. [`start`] hands the work to a thread, and the caller polls a status
//! and a pair of counters from whatever timer it already runs — no callback
//! crosses the language boundary, because a C callback that panics or unwinds on
//! a foreign thread is not something this library can make safe.
//!
//! Cancellation is checked before each object is planned, and once more before
//! the file is written. A cancelled job yields no G-code at all; it never returns
//! a partial file that a caller could send to a printer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use hfw_config::PrinterConfig;
use hfw_engine::{PlateOrder, Settings};
use hfw_plate::Plate;

use crate::{write_job, HfwGcode};

/// Where the job has got to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    /// Planning the plate, one object at a time.
    Slicing = 0,
    /// Writing and verifying the G-code.
    Writing = 1,
    /// Everything else is finished.
    Finished = 2,
}

/// The state a caller can read while the job runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    /// Where the job has got to, as a [`Stage`] value.
    pub stage: i32,
    /// Work done in the current stage.
    pub done: usize,
    /// Work to do in the current stage; zero until the stage is known.
    pub total: usize,
}

#[derive(Default)]
struct State {
    progress: Progress,
    gcode: Option<HfwGcode>,
    error: Option<String>,
    cancelled: bool,
    finished: bool,
}

#[derive(Default)]
struct Inner {
    state: Mutex<State>,
    ready: Condvar,
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("the job lock is not poisoned")
    }
}

/// A job running on its own thread.
pub struct Job {
    inner: Arc<Inner>,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// What a poll or wait reports.
pub const RUNNING: i32 = 0;
/// The job finished and its G-code is available.
pub const DONE: i32 = 1;
/// The job was cancelled and produced nothing.
pub const CANCELLED: i32 = 2;
/// The job failed; [`crate::hfw_last_error`] says why.
pub const FAILED: i32 = -1;

/// Starts a job for `plate` with `profile` applied.
///
/// The plate is copied into the job, so the caller may keep editing it — or free
/// it — while the job runs. Layer heights of zero take the profile's defaults,
/// exactly as a synchronous slice does.
pub fn start(
    plate: &Plate,
    profile: &str,
    first_layer_height: f64,
    layer_height: f64,
) -> Result<Job, String> {
    let profile_text = profile.to_owned();
    let plate = plate.clone();
    let inner = Arc::new(Inner {
        state: Mutex::new(State {
            progress: Progress {
                stage: Stage::Slicing as i32,
                done: 0,
                total: 0,
            },
            ..State::default()
        }),
        ready: Condvar::new(),
    });
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_state = Arc::clone(&inner);
    let worker_cancel = Arc::clone(&cancel);
    let thread = std::thread::Builder::new()
        .name("hfw-job".to_owned())
        .spawn(move || {
            let outcome = run(
                &plate,
                &profile_text,
                first_layer_height,
                layer_height,
                &worker_state,
                &worker_cancel,
            );
            let mut state = worker_state.lock();
            match outcome {
                Ok(gcode) => {
                    state.gcode = Some(gcode);
                    state.progress.stage = Stage::Finished as i32;
                }
                Err(Outcome::Cancelled) => state.cancelled = true,
                Err(Outcome::Failed(message)) => state.error = Some(message),
            }
            state.finished = true;
            drop(state);
            worker_state.ready.notify_all();
        })
        .map_err(|error| format!("the job thread could not start: {error}"))?;
    Ok(Job {
        inner,
        cancel,
        thread: Some(thread),
    })
}

enum Outcome {
    Cancelled,
    Failed(String),
}

fn run(
    plate: &Plate,
    profile: &str,
    first_layer_height: f64,
    layer_height: f64,
    state: &Arc<Inner>,
    cancel: &Arc<AtomicBool>,
) -> Result<HfwGcode, Outcome> {
    let cfg = PrinterConfig::load(std::path::Path::new(profile))
        .map_err(|error| Outcome::Failed(error.to_string()))?;
    let mut settings: Settings = plate.settings().clone();
    settings.apply_printer(&cfg);
    let (first, layer) = crate::heights_of(&cfg, first_layer_height, layer_height);
    let job = plate
        .slice_using_with_progress(
            &settings,
            first,
            layer,
            PlateOrder::ByLayer,
            |done, total| {
                let mut state = state.lock();
                state.progress.done = done;
                state.progress.total = total;
                drop(state);
                !cancel.load(Ordering::Relaxed)
            },
        )
        .map_err(|error| {
            if matches!(error, hfw_plate::PlateError::Cancelled) {
                Outcome::Cancelled
            } else {
                Outcome::Failed(error.to_string())
            }
        })?;
    if cancel.load(Ordering::Relaxed) {
        return Err(Outcome::Cancelled);
    }
    {
        let mut state = state.lock();
        state.progress.stage = Stage::Writing as i32;
        state.progress.done = 0;
        state.progress.total = job.layers.len();
    }
    let gcode = write_job(&job, &cfg).map_err(Outcome::Failed)?;
    {
        let mut state = state.lock();
        state.progress.done = state.progress.total;
    }
    Ok(gcode)
}

impl Job {
    /// Reports the status and copies the current progress into `out`.
    ///
    /// Does not block: it takes the job's lock for as long as it takes to read
    /// three numbers, and never waits for the worker.
    pub fn poll(&self) -> (i32, Progress) {
        let state = self.inner.lock();
        let status = status_of(&state);
        let progress = Progress {
            stage: state.progress.stage,
            done: state.progress.done,
            total: state.progress.total,
        };
        (status, progress)
    }

    /// Asks the job to stop. A job that has already finished is unaffected.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The message a failed job carries, if it failed.
    pub fn failure(&self) -> Option<String> {
        self.inner.lock().error.clone()
    }

    /// Blocks until the job has finished, been cancelled or failed.
    pub fn wait(&self) {
        let mut state = self.inner.lock();
        while !state.finished {
            state = self
                .inner
                .ready
                .wait(state)
                .expect("the job lock is not poisoned");
        }
    }

    /// Waits, then hands over the G-code if the job succeeded.
    pub fn take_gcode(self) -> Result<HfwGcode, String> {
        let mut state = self.inner.lock();
        if !state.finished {
            return Err("the job has not finished".to_owned());
        }
        if let Some(error) = state.error.take() {
            return Err(error);
        }
        if state.cancelled {
            return Err("the job was cancelled".to_owned());
        }
        state
            .gcode
            .take()
            .ok_or_else(|| "the job produced no G-code".to_owned())
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn status_of(state: &State) -> i32 {
    if state.cancelled {
        CANCELLED
    } else if state.error.is_some() {
        FAILED
    } else if state.finished {
        DONE
    } else {
        RUNNING
    }
}
