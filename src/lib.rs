#![doc=include_str!("../README.md")]

use event_listener::Event;
use event_listener::EventListener;
use nix::libc::c_int;
use nix::sys::signal::sigaction;
use nix::sys::signal::SaFlags;
use nix::sys::signal::SigAction;
use nix::sys::signal::SigHandler;
use nix::sys::signal::SigSet;
use once_cell::sync::Lazy;
use pin_project::pin_project;
use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Once;
use std::task::Context;
use std::task::Poll;

#[derive(Debug)]
struct UnsafeCellOptEvent(UnsafeCell<Option<Event>>);

unsafe impl Sync for UnsafeCellOptEvent {}

impl UnsafeCellOptEvent {
    fn new() -> Self {
        UnsafeCellOptEvent(UnsafeCell::new(None))
    }

    unsafe fn get(&self) -> &Option<Event> {
        unsafe { &*self.0.get() }
    }

    unsafe fn set(&self, event: Event) {
        let ptr = self.0.get();
        unsafe {
            *ptr = Some(event);
        }
    }
}

static REGISTERED_EVENTS: Lazy<Vec<(AtomicBool, Once, UnsafeCellOptEvent)>> = Lazy::new(|| {
    let n_signal = nix::libc::SIGRTMAX() as usize;
    (0..n_signal)
        .map(|_| {
            (
                AtomicBool::new(false),
                Once::new(),
                UnsafeCellOptEvent::new(),
            )
        })
        .collect()
});

extern "C" fn handler(sig_num: c_int) {
    let sig_num = sig_num as usize;
    debug_assert!(REGISTERED_EVENTS[sig_num].0.load(Ordering::Relaxed));
    unsafe {
        REGISTERED_EVENTS[sig_num]
            .2
            .get()
            .as_ref()
            .expect("event should be set")
            .notify(usize::MAX);
    }
}

pub use nix::sys::signal::Signal;

/// A future that would be resolved when received the specified signal.
///
/// # Examples
///
/// Wait for `SIGTERM`:
///
/// ```rust,no_run
/// # use monoio::FusionDriver;
/// # use nix::sys::signal::Signal;
/// # use signal_future::SignalFut;
/// #
/// # async {
/// let sigterm_fut = SignalFut::new(Signal::SIGTERM);
/// sigterm_fut.await;
/// # };
/// ```
#[pin_project]
pub struct SignalFut {
    signal: Signal,
    #[pin]
    listener: EventListener,
}

impl SignalFut {
    /// Create a `SignalFut` for `signal`.
    pub fn new(signal: Signal) -> SignalFut {
        let sig_num = signal as usize;
        if REGISTERED_EVENTS[sig_num].0.load(Ordering::Relaxed) {
            let listener = unsafe {
                REGISTERED_EVENTS[sig_num]
                    .2
                    .get()
                    .as_ref()
                    .expect("TODO")
                    .listen()
            };
            return SignalFut { signal, listener };
        }

        REGISTERED_EVENTS[sig_num].1.call_once(|| {
            // Create event
            let event = Event::new();
            unsafe {
                REGISTERED_EVENTS[sig_num].2.set(event);
            }

            // dispose the signal
            let sig_handler = SigHandler::Handler(handler);
            let sig_action = SigAction::new(sig_handler, SaFlags::empty(), SigSet::empty());
            // SAFETY:
            // if `event-listener::Event::notify(usize::MAX)` is signal-safe, then it is safe.
            unsafe { sigaction(signal, &sig_action).unwrap() };

            // Set the initialized mark
            REGISTERED_EVENTS[sig_num].0.store(true, Ordering::Relaxed);
        });

        if !REGISTERED_EVENTS[sig_num].0.load(Ordering::Relaxed) {
            panic!("failed to set signal handler");
        }

        let listener = unsafe {
            REGISTERED_EVENTS[sig_num]
                .2
                .get()
                .as_ref()
                .expect("TODO")
                .listen()
        };
        SignalFut { signal, listener }
    }
}

impl Future for SignalFut {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.listener.poll(cx)
    }
}

/// Completes when a “ctrl-c” notification is sent to the process.
pub async fn ctrl_c() {
    SignalFut::new(Signal::SIGINT).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    #[monoio::test]
    async fn sigint_with_monoio() {
        let fut = SignalFut::new(Signal::SIGINT);
        kill(Pid::this(), Signal::SIGINT).unwrap();
        fut.await;
    }

    #[tokio::test]
    async fn sigint_with_tokio() {
        let fut = SignalFut::new(Signal::SIGINT);
        kill(Pid::this(), Signal::SIGINT).unwrap();
        fut.await;
    }

    #[test]
    fn signt_with_futures_block_on() {
        futures::executor::block_on(async move {
            let fut = SignalFut::new(Signal::SIGINT);
            kill(Pid::this(), Signal::SIGINT).unwrap();
            fut.await;
        });
    }

    #[test]
    fn sigint_with_glommio() {
        glommio::LocalExecutorBuilder::default()
            .spawn(|| async {
                let fut = SignalFut::new(Signal::SIGINT);
                kill(Pid::this(), Signal::SIGINT).unwrap();
                fut.await;
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[tokio::test]
    async fn multiple_futures_with_tokio_select() {
        let sigint_fut = SignalFut::new(Signal::SIGINT);
        let sigquit_fut = SignalFut::new(Signal::SIGTERM);
        kill(Pid::this(), Signal::SIGTERM).unwrap();
        tokio::select! {
            _ = sigint_fut => {},
            _ = sigquit_fut => {},
        }
    }

    #[tokio::test]
    async fn multiple_tasks_waiting_for_same_signal() {
        let task1 = async {
            SignalFut::new(Signal::SIGINT).await;
        };
        let task2 = async {
            SignalFut::new(Signal::SIGINT).await;
        };

        let handle1 = tokio::spawn(task1);
        let handle2 = tokio::spawn(task2);

        // sleep for 1 second to ensure that task1 and task2 will be polled for
        // at least once so that signal handler can be set.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        kill(Pid::this(), Signal::SIGINT).unwrap();

        handle1.await.unwrap();
        handle2.await.unwrap();
    }
}
