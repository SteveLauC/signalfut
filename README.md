## Async signal handler

Similar to [tokio::signal][link], but can be used with [glommio][g] and [monoio][m].

[link]: https://docs.rs/tokio/latest/tokio/signal/index.html
[g]: https://github.com/DataDog/glommio
[m]: https://github.com/bytedance/monoio

## Examples

Greet when received either `SIGINT` or `SIGQUIT`:

```rust,no_run
use async_signal_handler::ctrl_c;
use async_signal_handler::Signal;
use async_signal_handler::SignalFut;

fn main() {
    let mut rt = monoio::RuntimeBuilder::<monoio::FusionDriver>::new()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        tokio::select! {
            _ = ctrl_c() => {},
            _ = SignalFut::new(Signal::SIGQUIT) => {},
        }
        println!("Greeting!");
    });
}
```
