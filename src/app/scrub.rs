//! Preview frames decoded off the UI thread: one worker thread per video
//! track (screen, camera), each with its own `VtDecoder`.
//!
//! The UI posts the source time it wants with `Worker::request`, and each
//! new time is a new generation. The worker decodes the newest one and
//! publishes the frame with that generation, or with a newer one that
//! resolves to the same sample. A walk that a newer request supersedes
//! still finishes and publishes with its own generation while the newer
//! time is within `SCRUB` of it (continuous scrubbing); after a farther jump
//! it stops and publishes nothing. Published generations only move forward.
//! While playing, the UI also names the next redraw's source time: once
//! idle, the worker decodes that sample ahead and keeps it unpublished, and
//! a request for a time that resolves to it publishes it within the call, so
//! the draw that asks for it shows it. A worker's publication raises the
//! shared `Signal`, which wakes the event loop once until the UI reads again.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::Result;

use crate::media::Frame;
use crate::media::vt::VtDecoder;

/// How far, in source ms, the newest request may be from a walk's time for
/// the walk to finish and show its frame. Dragging asks for a new time every
/// redraw, so without this a walk back to the keyframe would never finish;
/// farther away, the request is a jump and the walk stops.
const SCRUB: f64 = 1000.0;

/// Decoding attempts per generation, with a decoder reset between them.
const ATTEMPTS: usize = 2;

/// What a worker needs from its decoder: `VtDecoder`, or a fake in tests.
trait Decode: Send + 'static {
    type Frame: Clone + Send + 'static;
    /// The sample on screen at `source_ms`.
    fn target(&self, source_ms: f64) -> usize;
    /// The source times `target` resolves to sample `i`.
    fn span(&self, i: usize) -> Range<f64>;
    /// Decode sample `i`, asking `keep_going` between batches. `Ok(None)`
    /// means it stopped, keeping its progress for a later sample in the
    /// same GOP.
    fn decode(&mut self, i: usize, keep_going: impl Fn() -> bool) -> Result<Option<Self::Frame>>;
    /// Start over with a fresh session.
    fn reset(&mut self);
}

impl Decode for VtDecoder {
    type Frame = Frame;

    fn target(&self, source_ms: f64) -> usize {
        VtDecoder::target(self, source_ms)
    }

    fn span(&self, i: usize) -> Range<f64> {
        VtDecoder::span(self, i)
    }

    fn decode(&mut self, i: usize, keep_going: impl Fn() -> bool) -> Result<Option<Frame>> {
        VtDecoder::decode(self, i, keep_going)
    }

    fn reset(&mut self) {
        VtDecoder::reset(self)
    }
}

/// Wakes the UI when a worker publishes, once until the UI reads again.
/// Shared by a preview's workers.
pub struct Signal {
    pending: AtomicBool,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Signal {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Arc<Signal> {
        Arc::new(Signal { pending: AtomicBool::new(false), wake: Box::new(wake) })
    }

    /// Clear the flag, then `read` the workers' frames. A publication that
    /// `read` misses lands after the clear, so it wakes the UI again.
    pub fn read<T>(&self, read: impl FnOnce() -> T) -> T {
        self.pending.store(false, Ordering::SeqCst);
        read()
    }

    /// Only the first publication after a read wakes the UI.
    fn raise(&self) {
        if !self.pending.swap(true, Ordering::SeqCst) {
            (self.wake)();
        }
    }
}

/// A frame a worker published, with the generation it answers.
#[derive(Clone)]
pub struct Published<F> {
    pub generation: u64,
    /// Publications so far on this track: changes with every new frame.
    pub revision: u64,
    pub frame: F,
    /// The decoder's sample id of `frame`.
    sample: usize,
}

/// A sample the worker decoded ahead, unpublished until a request asks for
/// a time in `span`.
struct Prefetched<F> {
    sample: usize,
    span: Range<f64>,
    /// `None` when decoding it failed: a request for it decodes it again.
    frame: Option<F>,
}

/// A track's decode thread. Dropping it stops the decode in progress and
/// joins the thread.
pub struct Worker<F> {
    shared: Arc<Shared<F>>,
    thread: Option<JoinHandle<()>>,
}

struct Shared<F> {
    state: Mutex<State<F>>,
    /// New requests and stop, for the worker; settled requests, for `wait`.
    changed: Condvar,
    signal: Arc<Signal>,
}

struct State<F> {
    /// The newest request: generation and source time.
    request: Option<(u64, f64)>,
    /// The next redraw's source time while playing, decoded when idle.
    ahead: Option<f64>,
    prefetched: Option<Prefetched<F>>,
    /// The newest generation the worker finished, published or failed.
    done: u64,
    published: Option<Published<F>>,
    /// The generation that failed every attempt, until a frame is published.
    failed: Option<u64>,
    /// The decoder didn't open: the track never gets a frame.
    broken: bool,
    stop: bool,
    /// Failures logged.
    #[cfg(test)]
    logged: usize,
    /// Every publication as (generation, frame).
    #[cfg(test)]
    history: Vec<(u64, F)>,
    /// The worker sleeps with nothing to do.
    #[cfg(test)]
    idle: bool,
}

impl<F> Default for State<F> {
    fn default() -> Self {
        State {
            request: None,
            ahead: None,
            prefetched: None,
            done: 0,
            published: None,
            failed: None,
            broken: false,
            stop: false,
            #[cfg(test)]
            logged: 0,
            #[cfg(test)]
            history: vec![],
            #[cfg(test)]
            idle: false,
        }
    }
}

impl Worker<Frame> {
    /// Decode the first video track of `path` on a new thread. The decoder,
    /// with its sample index, opens on that thread; if it can't, the track
    /// stays without a frame.
    pub fn open(path: PathBuf, track: &'static str, signal: Arc<Signal>) -> Self {
        Worker::spawn(track, signal, move || objc2::rc::autoreleasepool(|_| VtDecoder::open(&path)))
    }
}

impl<F: Clone + Send + 'static> Worker<F> {
    fn spawn<D: Decode<Frame = F>>(track: &'static str, signal: Arc<Signal>, open: impl FnOnce() -> Result<D> + Send + 'static) -> Self {
        let shared = Arc::new(Shared { state: Mutex::default(), changed: Condvar::new(), signal });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name(format!("glide-preview-{track}"))
            .spawn(move || match open() {
                Ok(dec) => run(dec, &s, track),
                Err(e) => {
                    s.report(track, &e);
                    s.lock().broken = true;
                    s.changed.notify_all();
                }
            })
            .expect("spawn a preview decode thread");
        Worker { shared, thread: Some(thread) }
    }

    /// Ask for the frame at `source_ms`. While playing, `ahead` is the next
    /// redraw's source time, to decode before it's asked for; `None`
    /// otherwise. Asking for the same time again starts no new generation.
    /// If the frame was decoded ahead, it's published before this returns,
    /// for the caller's next read; that doesn't wake the UI.
    pub fn request(&self, source_ms: f64, ahead: Option<f64>) {
        let mut s = self.shared.lock();
        let moved = s.request.is_none_or(|(_, t)| t != source_ms);
        if !moved && s.ahead == ahead {
            return;
        }
        if moved {
            let generation = s.request.map_or(1, |(g, _)| g + 1);
            s.request = Some((generation, source_ms));
            if let Some(Prefetched { sample, frame: Some(frame), .. }) = s.prefetched.take_if(|p| p.frame.is_some() && p.span.contains(&source_ms)) {
                s.show(generation, sample, frame);
            }
        }
        s.ahead = ahead;
        drop(s);
        self.shared.changed.notify_all();
    }

    /// The latest published frame. Read it inside `Signal::read`.
    pub fn latest(&self) -> Option<Published<F>> {
        self.shared.lock().published.clone()
    }

    /// Block until the newest request is published or failed, or until
    /// `deadline`; returns whether it settled. For callers without an event
    /// loop to wake (tests, benchmarks).
    pub fn wait(&self, deadline: Instant) -> bool {
        let mut s = self.shared.lock();
        loop {
            if s.broken || s.request.is_none_or(|(g, _)| s.done >= g) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            s = self.shared.changed.wait_timeout(s, left).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

impl<F> Drop for Worker<F> {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.changed.notify_all();
        // The decode in progress stops at its next batch; the decoder drains
        // its session when the thread drops it.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl<F: Clone> State<F> {
    /// Make `frame`, sample `sample`, the published frame for `generation`.
    fn show(&mut self, generation: u64, sample: usize, frame: F) {
        #[cfg(test)]
        self.history.push((generation, frame.clone()));
        let revision = self.published.as_ref().map_or(0, |p| p.revision) + 1;
        self.published = Some(Published { generation, revision, frame, sample });
        self.done = self.done.max(generation);
        self.failed = None;
    }
}

impl<F> Shared<F> {
    fn lock(&self) -> MutexGuard<'_, State<F>> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Publish `frame`, sample `i` decoded for `generation` at source time
    /// `t`. It carries the newest generation if that resolves to the same
    /// sample, else its own while the newest time is within `SCRUB` of `t`.
    /// It's dropped after a jump, or when it's no newer than the published
    /// frame.
    fn publish(&self, generation: u64, t: f64, i: usize, frame: F, target: impl Fn(f64) -> usize) -> bool
    where
        F: Clone,
    {
        let mut s = self.lock();
        let Some((newest, newest_t)) = s.request else { return false };
        let generation = if newest == generation || target(newest_t) == i {
            newest
        } else if (newest_t - t).abs() <= SCRUB {
            generation
        } else {
            return false;
        };
        if s.published.as_ref().is_some_and(|p| p.generation >= generation) {
            return false;
        }
        s.show(generation, i, frame);
        // Raised before unlocking: whoever sees the frame settled also sees
        // the wake.
        self.signal.raise();
        drop(s);
        self.changed.notify_all();
        true
    }

    fn superseded(&self, generation: u64) -> bool {
        self.lock().request.is_some_and(|(g, _)| g != generation)
    }

    /// `generation` failed every attempt: settle it without a frame, unless
    /// a newer request superseded it (that one gets its own attempts). Only
    /// the first failure since a frame was last shown is logged.
    fn fail(&self, track: &str, generation: u64, e: &anyhow::Error) {
        let mut s = self.lock();
        if s.request.is_some_and(|(g, _)| g != generation) {
            return;
        }
        if s.failed.is_none() {
            eprintln!("glide: {track} preview: {e:#}");
            #[cfg(test)]
            {
                s.logged += 1;
            }
        }
        s.done = s.done.max(generation);
        s.failed = Some(generation);
        drop(s);
        self.changed.notify_all();
    }

    fn report(&self, track: &str, e: &anyhow::Error) {
        eprintln!("glide: {track} preview: {e:#}");
        #[cfg(test)]
        {
            self.lock().logged += 1;
        }
    }
}

/// What the worker does next.
enum Job {
    /// Decode and publish the newest request: generation and source time.
    Request(u64, f64),
    /// Decode this sample ahead, into `State::prefetched`.
    Ahead(usize),
}

/// The worker thread: decode the newest request until stopped, and the next
/// redraw's sample while idle.
fn run<D: Decode>(mut dec: D, shared: &Shared<D::Frame>, track: &str) {
    loop {
        let job = {
            let mut s = shared.lock();
            loop {
                if s.stop {
                    return;
                }
                if let Some((g, t)) = s.request
                    && g > s.done
                {
                    break Job::Request(g, t);
                }
                if let Some(t) = s.ahead {
                    let i = dec.target(t);
                    if s.published.as_ref().is_none_or(|p| p.sample != i) && s.prefetched.as_ref().is_none_or(|p| p.sample != i) {
                        break Job::Ahead(i);
                    }
                }
                #[cfg(test)]
                {
                    s.idle = true;
                    shared.changed.notify_all();
                }
                s = shared.changed.wait(s).unwrap_or_else(|e| e.into_inner());
                #[cfg(test)]
                {
                    s.idle = false;
                }
            }
        };
        let (generation, t) = match job {
            Job::Request(g, t) => (g, t),
            Job::Ahead(i) => {
                // A request takes over at the next batch, continuing this
                // walk if it's ahead in the same GOP.
                let idle = || {
                    let s = shared.lock();
                    !s.stop && s.request.is_none_or(|(g, _)| g <= s.done)
                };
                let frame = match dec.decode(i, idle) {
                    Ok(Some(f)) => Some(f),
                    Ok(None) => continue,
                    // Not retried: a request for it decodes it on its own.
                    Err(_) => None,
                };
                shared.lock().prefetched = Some(Prefetched { sample: i, span: dec.span(i), frame });
                continue;
            }
        };
        let i = dec.target(t);
        // The published frame serves any request that resolves to its
        // sample; so does the one decoded ahead, if the request came before
        // it was ready.
        let ready = {
            let mut s = shared.lock();
            match s.published.as_ref().filter(|p| p.sample == i) {
                Some(p) => Some(p.frame.clone()),
                None => s.prefetched.take_if(|p| p.sample == i).and_then(|p| p.frame),
            }
        };
        let frame = match ready {
            Some(f) => Some(f),
            None => decode(&mut dec, shared, track, (generation, t), i),
        };
        if let Some(f) = frame {
            shared.publish(generation, t, i, f, |t| dec.target(t));
        }
    }
}

/// Decode sample `i` for the request (generation, source time), retrying
/// once. `None` when a jump stopped it, or when it failed (settled with
/// `Shared::fail`).
fn decode<D: Decode>(dec: &mut D, shared: &Shared<D::Frame>, track: &str, (generation, t): (u64, f64), i: usize) -> Option<D::Frame> {
    let mut attempt = 1;
    loop {
        // Go on while the newest request is within `SCRUB` of this one (it
        // is this one until something newer comes); after a jump, stop after
        // this batch. The newer request decodes next, continuing this walk
        // if it's ahead in the same GOP.
        let keep_going = || {
            let s = shared.lock();
            !s.stop && s.request.is_some_and(|(_, newest)| (newest - t).abs() <= SCRUB)
        };
        match dec.decode(i, keep_going) {
            Ok(frame) => return frame,
            Err(_) if attempt < ATTEMPTS && !shared.superseded(generation) => {
                attempt += 1;
                dec.reset();
            }
            Err(e) => {
                shared.fail(track, generation, &e);
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use anyhow::bail;

    use super::*;

    const GOP: usize = 32;
    const BATCH: usize = 8;
    /// Source ms per sample.
    const STEP: f64 = 10.0;

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    /// Source time of sample `i`.
    fn at(i: usize) -> f64 {
        i as f64 * STEP
    }

    #[derive(Clone, Debug, PartialEq)]
    enum Ev {
        /// A `decode` call for this sample.
        Call(usize),
        /// Hidden frames decoded in one batch.
        Batch(usize, usize),
        /// The target decoded with output.
        Output(usize),
        Failed,
        Reset,
    }

    /// Blocks the fake decoder before each batch and output while closed,
    /// letting one through per `release`.
    #[derive(Default)]
    struct Gate {
        state: Mutex<(bool, usize, usize)>,
        cv: Condvar,
    }

    impl Gate {
        fn close(&self) {
            self.state.lock().unwrap().0 = true;
        }

        fn open(&self) {
            self.state.lock().unwrap().0 = false;
            self.cv.notify_all();
        }

        fn release(&self) {
            self.state.lock().unwrap().2 += 1;
            self.cv.notify_all();
        }

        /// Wait until `n` steps in all have reached the closed gate.
        fn reached(&self, n: usize) {
            let s = self.state.lock().unwrap();
            let (s, timeout) = self.cv.wait_timeout_while(s, Duration::from_secs(10), |s| s.1 < n).unwrap();
            assert!(!timeout.timed_out(), "the decoder reached the gate {} times, not {n}", s.1);
        }

        fn pass(&self) {
            let mut s = self.state.lock().unwrap();
            if !s.0 {
                return;
            }
            s.1 += 1;
            self.cv.notify_all();
            // A test that never releases fails on its own deadline.
            let (mut s, _) = self.cv.wait_timeout_while(s, Duration::from_secs(10), |s| s.0 && s.2 == 0).unwrap();
            if s.0 {
                s.2 = s.2.saturating_sub(1);
            }
        }
    }

    /// Shared between a test and its fake decoder.
    #[derive(Default)]
    struct Ctl {
        log: Mutex<Vec<Ev>>,
        gate: Gate,
        /// Calls left to fail.
        failures: AtomicUsize,
        dropped: AtomicBool,
    }

    impl Ctl {
        fn log(&self) -> Vec<Ev> {
            self.log.lock().unwrap().clone()
        }

        fn count(&self, f: impl Fn(&Ev) -> bool) -> usize {
            self.log().iter().filter(|e| f(e)).count()
        }

        fn push(&self, e: Ev) {
            self.log.lock().unwrap().push(e);
        }
    }

    /// GOPs of `GOP` samples `STEP` ms apart, decoded in batches of `BATCH`
    /// like `VtDecoder`, continuing a walk forward in the same GOP. Frames
    /// are sample ids.
    struct Fake {
        ctl: Arc<Ctl>,
        /// The walk's GOP start and last decoded sample.
        decoded: Option<(usize, usize)>,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.ctl.dropped.store(true, Ordering::SeqCst);
        }
    }

    impl Decode for Fake {
        type Frame = usize;

        fn target(&self, source_ms: f64) -> usize {
            (source_ms / STEP).floor().max(0.0) as usize
        }

        fn span(&self, i: usize) -> Range<f64> {
            (if i == 0 { f64::NEG_INFINITY } else { at(i) })..at(i + 1)
        }

        fn decode(&mut self, i: usize, keep_going: impl Fn() -> bool) -> Result<Option<usize>> {
            self.ctl.push(Ev::Call(i));
            // Only this thread changes it once the test set it.
            let failures = self.ctl.failures.load(Ordering::SeqCst);
            if failures > 0 {
                self.ctl.failures.store(failures - 1, Ordering::SeqCst);
                self.ctl.push(Ev::Failed);
                self.decoded = None;
                bail!("injected failure at sample {i}");
            }
            let start = i / GOP * GOP;
            let from = match self.decoded {
                Some((s, last)) if s == start && last < i => last + 1,
                _ => start,
            };
            let hidden: Vec<usize> = (from..i).collect();
            for batch in hidden.chunks(BATCH) {
                self.ctl.gate.pass();
                let last = batch[batch.len() - 1];
                self.ctl.push(Ev::Batch(batch[0], last));
                self.decoded = Some((start, last));
                if !keep_going() {
                    return Ok(None);
                }
            }
            self.ctl.gate.pass();
            self.ctl.push(Ev::Output(i));
            self.decoded = Some((start, i));
            Ok(Some(i))
        }

        fn reset(&mut self) {
            self.ctl.push(Ev::Reset);
            self.decoded = None;
        }
    }

    struct Rig {
        ctl: Arc<Ctl>,
        signal: Arc<Signal>,
        wakes: Arc<AtomicUsize>,
        worker: Worker<usize>,
    }

    impl Rig {
        fn new() -> Rig {
            Rig::with(Ok(()))
        }

        fn with(open: Result<()>) -> Rig {
            let ctl = Arc::new(Ctl::default());
            let wakes = Arc::new(AtomicUsize::new(0));
            let w = wakes.clone();
            let signal = Signal::new(move || {
                w.fetch_add(1, Ordering::SeqCst);
            });
            let c = ctl.clone();
            let worker = Worker::spawn("test", signal.clone(), move || open.map(|_| Fake { ctl: c, decoded: None }));
            Rig { ctl, signal, wakes, worker }
        }

        /// Ask for sample `i`'s time, not playing.
        fn request(&self, i: usize) {
            self.worker.request(at(i), None);
        }

        fn settle(&self) {
            assert!(self.worker.wait(deadline()), "the worker didn't settle");
        }

        /// Wait until the worker sleeps with nothing left to do.
        fn idle(&self) {
            let s = self.worker.shared.lock();
            let (_s, timeout) = self.worker.shared.changed.wait_timeout_while(s, Duration::from_secs(10), |s| !s.idle).unwrap();
            assert!(!timeout.timed_out(), "the worker never went idle");
        }

        fn history(&self) -> Vec<(u64, usize)> {
            self.worker.shared.lock().history.clone()
        }

        fn wakes(&self) -> usize {
            self.wakes.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn converges_to_the_newest_target() {
        let r = Rig::new();
        r.ctl.gate.close();
        r.request(40);
        r.ctl.gate.reached(1);
        // Jumps: both are farther than `SCRUB` from 40.
        r.request(400);
        r.request(250);
        r.ctl.gate.open();
        r.settle();
        assert_eq!(r.history(), vec![(3, 250)]);
        assert_eq!(r.ctl.count(|e| matches!(e, Ev::Output(_))), 1);
    }

    #[test]
    fn a_jump_preempts_a_walk_between_batches() {
        let r = Rig::new();
        r.ctl.gate.close();
        // Four batches of hidden frames before sample 60.
        r.request(60);
        r.ctl.gate.reached(1);
        r.request(200);
        r.ctl.gate.open();
        r.settle();
        let log = r.ctl.log();
        let walk: Vec<_> = log.iter().take_while(|e| **e != Ev::Call(200)).collect();
        assert_eq!(walk, [&Ev::Call(60), &Ev::Batch(32, 39)], "the old walk stops after its batch");
        assert_eq!(r.history(), vec![(2, 200)]);
    }

    #[test]
    fn a_slow_old_frame_never_lands_after_a_jump() {
        let r = Rig::new();
        r.ctl.gate.close();
        // A full-sync sample: straight to the output decode, which can't stop.
        r.request(32);
        r.ctl.gate.reached(1);
        r.request(200);
        r.ctl.gate.open();
        r.settle();
        assert!(r.ctl.log().contains(&Ev::Output(32)), "the old frame was decoded");
        assert_eq!(r.history(), vec![(2, 200)]);
    }

    #[test]
    fn scrubbing_back_shows_finished_walks_and_converges() {
        let r = Rig::new();
        r.ctl.gate.close();
        // Dragging back one sample per step from a GOP's end: each walk
        // starts at the keyframe, and a new request lands at every batch.
        let target = |n: usize| 127 - n;
        let steps = 24;
        r.request(target(0));
        for n in 1..=steps {
            r.ctl.gate.reached(n);
            r.request(target(n));
            r.ctl.gate.release();
        }
        let dragging = r.history();
        r.ctl.gate.open();
        r.settle();
        let history = r.history();
        assert!(dragging.len() >= 3, "finished walks shown while dragging: {dragging:?}");
        assert!(history.windows(2).all(|w| w[0].0 < w[1].0), "generations went backwards: {history:?}");
        // Generation g asked for target(g - 1).
        assert!(history.iter().all(|&(g, i)| i == target(g as usize - 1)), "a frame under the wrong generation: {history:?}");
        assert_eq!(history.last(), Some(&(steps as u64 + 1, target(steps))));
    }

    #[test]
    fn a_finished_walk_never_replaces_a_newer_frame() {
        let shared = Shared { state: Mutex::default(), changed: Condvar::new(), signal: Signal::new(|| {}) };
        let target = |t: f64| (t / STEP) as usize;
        shared.lock().request = Some((3, at(45)));
        assert!(shared.publish(3, at(45), 45, 45, target));
        // Generation 2's walk finishes after 3 was published, close to the
        // newest request.
        shared.lock().request = Some((4, at(47)));
        assert!(!shared.publish(2, at(44), 44, 44, target));
        assert_eq!(shared.lock().history, vec![(3, 45)]);
    }

    /// Show 40 while sample 41 is decoded ahead, and wait until it's ready.
    fn prefetched_41() -> Rig {
        let r = Rig::new();
        r.ctl.gate.close();
        // 40 takes one batch and its output, then 41 its output.
        r.worker.request(at(40), Some(at(41)));
        r.ctl.gate.reached(1);
        r.ctl.gate.release();
        r.ctl.gate.reached(2);
        r.ctl.gate.release();
        r.settle();
        // The UI reads 40 before the prefetch finishes.
        assert_eq!(r.signal.read(|| r.worker.latest().map(|p| p.frame)), Some(40));
        r.ctl.gate.reached(3);
        r.ctl.gate.open();
        r.idle();
        assert_eq!(r.ctl.log().last(), Some(&Ev::Output(41)));
        r
    }

    #[test]
    fn a_prefetch_stays_private_until_requested() {
        let r = prefetched_41();
        assert_eq!(r.history(), vec![(1, 40)]);
        assert_eq!(r.signal.read(|| r.worker.latest().map(|p| (p.frame, p.revision))), Some((40, 1)));
        assert_eq!(r.wakes(), 1, "the prefetch woke the UI");
    }

    #[test]
    fn a_request_for_the_prefetched_sample_shows_it_at_once() {
        let r = prefetched_41();
        // Anywhere in 41's span, as the next draw asks for it.
        let seen = r.signal.read(|| {
            r.worker.request(at(41) + 7.0, None);
            r.worker.latest().map(|p| (p.generation, p.frame))
        });
        assert_eq!(seen, Some((2, 41)), "the draw that asked for 41 didn't get it");
        assert_eq!(r.history(), vec![(1, 40), (2, 41)]);
        r.settle();
        r.idle();
        assert_eq!(r.ctl.count(|e| *e == Ev::Call(41)), 1, "41 was decoded again");
        assert_eq!(r.wakes(), 1, "the handoff woke the UI");
        // It's now the published frame: another time in its span reuses it.
        r.request(41);
        r.settle();
        assert_eq!(r.history(), vec![(1, 40), (2, 41), (3, 41)]);
        assert_eq!(r.ctl.count(|e| *e == Ev::Call(41)), 1);
    }

    #[test]
    fn a_request_outside_the_prefetched_span_decodes() {
        let r = prefetched_41();
        // Hold the decode, so the read can't race it.
        r.ctl.gate.close();
        let seen = r.signal.read(|| {
            r.request(42);
            r.worker.latest().map(|p| p.frame)
        });
        assert_eq!(seen, Some(40));
        r.ctl.gate.open();
        r.settle();
        assert_eq!(r.history(), vec![(1, 40), (2, 42)]);
        assert_eq!(r.ctl.count(|e| *e == Ev::Call(42)), 1);
        assert_eq!(r.wakes(), 2);
    }

    #[test]
    fn a_request_preempts_a_prefetch() {
        let r = Rig::new();
        r.ctl.gate.close();
        // 40 takes one batch and its output; 60 after it three batches.
        r.worker.request(at(40), Some(at(60)));
        r.ctl.gate.reached(1);
        r.ctl.gate.release();
        r.ctl.gate.reached(2);
        r.ctl.gate.release();
        r.settle();
        // The prefetch's first batch.
        r.ctl.gate.reached(3);
        r.request(200);
        r.ctl.gate.open();
        r.settle();
        let log = r.ctl.log();
        let prefetch: Vec<_> = log.iter().skip_while(|e| **e != Ev::Call(60)).take_while(|e| **e != Ev::Call(200)).collect();
        assert_eq!(prefetch, [&Ev::Call(60), &Ev::Batch(41, 48)], "the prefetch stops after its batch");
        assert!(!log.contains(&Ev::Output(60)));
        assert_eq!(r.history(), vec![(1, 40), (2, 200)]);
    }

    #[test]
    fn one_failure_recovers_on_the_retry() {
        let r = Rig::new();
        r.ctl.failures.store(1, Ordering::SeqCst);
        r.request(40);
        r.settle();
        assert_eq!(r.history(), vec![(1, 40)]);
        assert_eq!(r.ctl.log()[..3], [Ev::Call(40), Ev::Failed, Ev::Reset]);
        let s = r.worker.shared.lock();
        assert_eq!((s.failed, s.logged), (None, 0));
    }

    #[test]
    fn a_permanent_failure_is_logged_once() {
        let r = Rig::new();
        r.ctl.failures.store(usize::MAX, Ordering::SeqCst);
        r.request(40);
        r.settle();
        assert_eq!(r.ctl.count(|e| matches!(e, Ev::Call(_))), ATTEMPTS);
        assert_eq!(r.ctl.count(|e| *e == Ev::Reset), 1);
        r.request(70);
        r.settle();
        let s = r.worker.shared.lock();
        assert_eq!((s.failed, s.logged, s.published.is_none()), (Some(2), 1, true));
    }

    #[test]
    fn a_failure_after_a_handed_over_frame_is_logged_again() {
        let r = prefetched_41();
        r.ctl.failures.store(ATTEMPTS, Ordering::SeqCst);
        r.request(200);
        r.settle();
        // The frame decoded ahead is shown again: the failures ended.
        r.request(41);
        assert_eq!(r.history().last(), Some(&(3, 41)));
        r.ctl.failures.store(ATTEMPTS, Ordering::SeqCst);
        r.request(300);
        r.settle();
        let s = r.worker.shared.lock();
        assert_eq!((s.failed, s.logged), (Some(4), 2));
    }

    #[test]
    fn a_superseded_failure_changes_nothing() {
        let shared: Shared<usize> = Shared { state: Mutex::default(), changed: Condvar::new(), signal: Signal::new(|| {}) };
        // Generation 2 failed, but 3 came in (and may already show a frame).
        shared.lock().request = Some((3, at(41)));
        shared.fail("test", 2, &anyhow::anyhow!("injected"));
        let s = shared.lock();
        assert_eq!((s.failed, s.logged, s.done), (None, 0, 0));
    }

    #[test]
    fn a_decoder_that_doesnt_open_breaks_the_track() {
        let r = Rig::with(Err(anyhow::anyhow!("no video track")));
        r.request(40);
        r.settle();
        let s = r.worker.shared.lock();
        assert_eq!((s.broken, s.logged, s.published.is_none()), (true, 1, true));
    }

    #[test]
    fn publications_wake_the_ui_once_until_it_reads() {
        let r = Rig::new();
        r.request(40);
        r.settle();
        assert_eq!(r.wakes(), 1);
        r.request(70);
        r.settle();
        assert_eq!(r.wakes(), 1, "still pending");
        assert_eq!(r.signal.read(|| r.worker.latest().map(|p| p.frame)), Some(70));
        r.request(100);
        r.settle();
        assert_eq!(r.wakes(), 2);
    }

    #[test]
    fn a_publication_during_a_read_is_not_stranded() {
        let r = Rig::new();
        r.request(40);
        r.settle();
        // The UI was woken for 40 and reads; 70 lands right after its read.
        let seen = r.signal.read(|| {
            let seen = r.worker.latest().map(|p| p.frame);
            r.request(70);
            r.settle();
            seen
        });
        let wakes = r.wakes();
        assert!(seen == Some(70) || wakes == 2, "saw {seen:?} with {wakes} wakes: 70 was stranded");
    }

    #[test]
    fn requests_for_the_same_sample_decode_it_once() {
        let r = Rig::new();
        r.ctl.gate.close();
        r.request(40);
        r.ctl.gate.reached(1);
        r.worker.request(at(40) + 5.0, None);
        r.ctl.gate.open();
        r.settle();
        // Repeating a time is no new generation.
        r.worker.request(at(40) + 5.0, None);
        r.worker.request(at(40) + 9.0, None);
        r.settle();
        assert_eq!(r.history(), vec![(2, 40), (3, 40)]);
        assert_eq!(r.ctl.count(|e| matches!(e, Ev::Output(_))), 1);
        assert_eq!(r.ctl.count(|e| matches!(e, Ev::Batch(..))), 1);
    }

    #[test]
    fn dropping_the_worker_joins_it() {
        let r = Rig::new();
        r.ctl.gate.close();
        r.request(60);
        r.ctl.gate.reached(1);
        let ctl = r.ctl.clone();
        let opener = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            ctl.gate.open();
        });
        drop(r.worker);
        assert!(r.ctl.dropped.load(Ordering::SeqCst), "the decoder outlived the worker");
        opener.join().unwrap();
    }
}
