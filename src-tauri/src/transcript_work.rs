//! One allocation thread and one admitted source lifetime for retained work.
//!
//! Inventory and root-graph orchestration stay on their existing runners. Only
//! a bounded source unit enters here; callers drop normalized rows before the
//! source scope returns. Never wait here while holding an index writer lock.
use std::cell::Cell;
use std::sync::{Condvar, Mutex, OnceLock, mpsc};

#[cfg(test)]
thread_local! { pub(crate) static DECODE_COUNT: Cell<usize> = const { Cell::new(0) }; }

thread_local! { static ON_DECODER: Cell<bool> = const { Cell::new(false) }; }

type Job = Box<dyn FnOnce() + Send>;

fn decoder() -> &'static mpsc::SyncSender<Job> {
    static WORKER: OnceLock<mpsc::SyncSender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(1);
        std::thread::Builder::new()
            .name("transcript-decode".into())
            .spawn(move || {
                ON_DECODER.set(true);
                for job in rx {
                    job();
                }
            })
            .expect("start transcript decoder");
        tx
    })
}

/// Decode and discard raw bytes/JSON on one persistent allocator thread.
pub(crate) fn decode<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    assert!(
        !ON_DECODER.get(),
        "transcript decoder cannot submit to itself"
    );
    #[cfg(test)]
    DECODE_COUNT.set(DECODE_COUNT.get() + 1);
    let (tx, rx) = mpsc::sync_channel(0);
    decoder()
        .send(Box::new(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            let _ = tx.send(result);
        }))
        .expect("transcript decoder stopped");
    match rx.recv().expect("transcript decoder lost result") {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

thread_local! { static IN_SOURCE: Cell<bool> = const { Cell::new(false) }; }
static ADMISSION: (Mutex<(u64, u64)>, Condvar) = (Mutex::new((0, 0)), Condvar::new());

#[cfg(test)]
pub(crate) fn source_admission_held_for_test() -> bool {
    IN_SOURCE.get()
}

struct SourceScope;
impl Drop for SourceScope {
    fn drop(&mut self) {
        IN_SOURCE.set(false);
        ADMISSION.0.lock().unwrap().1 += 1;
        ADMISSION.1.notify_all();
    }
}

/// FIFO admission covers read, decode, both consumer commits, and output drop.
/// Nested synchronous consumer calls reuse the owning source scope.
pub(crate) fn with_source<T>(work: impl FnOnce() -> T) -> T {
    assert!(!ON_DECODER.get(), "source admission must precede decoding");
    if IN_SOURCE.get() {
        return work();
    }
    let mut admission = ADMISSION.0.lock().unwrap();
    let ticket = admission.0;
    admission.0 += 1;
    while admission.1 != ticket {
        admission = ADMISSION.1.wait(admission).unwrap();
    }
    drop(admission);
    IN_SOURCE.set(true);
    let _scope = SourceScope;
    work()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // @lat: [[transcript-memory-tests#Transcript Memory Test Specs#Source Lifetime And Fairness]]
    #[test]
    fn source_lifetime_is_bounded_through_stalled_consumption_and_decoder_survives_panic() {
        let (entered, observed) = mpsc::channel();
        let (release, stalled) = mpsc::channel();
        let first = std::thread::spawn(move || {
            with_source(|| {
                let output = decode(|| vec![7_u8; 1024 * 1024]);
                entered.send(std::thread::current().id()).unwrap();
                stalled.recv().unwrap();
                assert_eq!(output.len(), 1024 * 1024);
            })
        });
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let (done, completed) = mpsc::channel();
        let next = std::thread::spawn(move || {
            with_source(|| {
                let worker = decode(|| std::thread::current().id());
                assert_eq!(worker, decode(|| std::thread::current().id()));
                done.send(()).unwrap();
            })
        });
        assert!(
            completed.recv_timeout(Duration::from_millis(50)).is_err(),
            "a normalized output still being consumed owns the budget"
        );
        release.send(()).unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        first.join().unwrap();
        next.join().unwrap();
        assert!(
            std::panic::catch_unwind(|| with_source(|| decode(|| panic!(
                "synthetic parser failure"
            ))))
            .is_err()
        );
        assert!(std::panic::catch_unwind(|| decode(|| decode(|| 42))).is_err());
        assert!(
            std::panic::catch_unwind(|| with_source(|| decode(|| with_source(|| 42)))).is_err()
        );
        assert_eq!(with_source(|| decode(|| 42)), 42);
    }
}
