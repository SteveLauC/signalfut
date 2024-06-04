## signal-future

Similar to [tokio::signal][link], but can be used with [glommio][g] and [monoio][m].

[link]: https://docs.rs/tokio/latest/tokio/signal/index.html
[g]: https://github.com/DataDog/glommio
[m]: https://github.com/bytedance/monoio

## Examples

Greet when receive either `SIGINT` or `SIGQUIT`:

```rust,no_run
use signal_future::ctrl_c;
use signal_future::Signal;
use signal_future::SignalFut;

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
