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
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::RwLock;
use std::task::Context;
use std::task::Poll;

static REGISTERED_EVENTS: Lazy<RwLock<HashMap<Signal, Event>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

extern "C" fn handler(sig_num: c_int) {
    let signal = Signal::try_from(sig_num).expect("unknown signal");
    REGISTERED_EVENTS
        .read()
        .unwrap()
        .get(&signal)
        .expect("should be registered")
        .notify(usize::MAX);
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
/// # use async_signal_handler::SignalFut;
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
        let mut registered_events_write_guard = REGISTERED_EVENTS.write().unwrap();
        match registered_events_write_guard.entry(signal) {
            Entry::Occupied(event) => {
                let listener = event.get().listen();
                SignalFut { signal, listener }
            }
            Entry::Vacant(key) => {
                let event = Event::new();
                let listener = event.listen();

                let sig_handler = SigHandler::Handler(handler);
                let sig_action = SigAction::new(sig_handler, SaFlags::empty(), SigSet::empty());
                // SAFETY: let's just assume it is safe
                unsafe { sigaction(signal, &sig_action).unwrap() };
                key.insert(event);
                SignalFut { signal, listener }
            }
        }
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
