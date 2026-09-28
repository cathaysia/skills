# Rust Rules

## 1. Observer / Event Propagation — Use a `Listener` Trait

When an object needs to propagate internal events outward, define a `Listener` trait and accept it as a generic or trait object. This keeps the object's internals private while letting callers observe behavior.

```rust
pub trait Listener: Send + Sync {
    fn on_event(&self, event: Event);
}

pub struct Worker<L: Listener> {
    listener: L,
}
```

Never expose internal channels, callbacks, or state directly — route all outbound signals through the `Listener` interface.

## 2. Null Object — Use `DummyListener`, Not `Option<Box<dyn Listener>>`

When no listener is needed, use a no-op `DummyListener` struct that implements `Listener`. This eliminates `if let Some(...)` guards everywhere and keeps call sites clean.

```rust
pub struct DummyListener;
impl Listener for DummyListener {
    fn on_event(&self, _event: Event) {}
}

// Usage — no Option unwrapping needed:
let worker = Worker::new(DummyListener);
```

Never use `Option<Box<dyn Listener>>` as a substitute for an optional listener.

## 3. Trait Object Ownership — `Arc<dyn T>` for Shared, `Box<dyn T>` for Exclusive

| Situation | Use |
|---|---|
| Shared ownership across threads | `Arc<dyn T>` |
| Single owner, heap-allocated | `Box<dyn T>` |
| No polymorphism needed | Generic `T: Trait` (see rule 4) |

## 4. Prefer Generics Over `dyn T` When Polymorphism Is Unnecessary

If a type or function only ever works with one concrete type at each call site, use a generic parameter instead of a trait object. Monomorphization is zero-cost, avoids vtable overhead, and enables inlining.

```rust
// Prefer this:
fn process<L: Listener>(listener: &L) { ... }

// Over this (only when runtime dispatch is actually needed):
fn process(listener: &dyn Listener) { ... }
```

Use `dyn T` only when the concrete type is unknown at compile time or when you need to store heterogeneous types in a collection.

## 5. Encapsulation — Keep Interfaces Simple, Complexity Inside

The public API of a type should be minimal and stable. Implementation complexity (state machines, retries, locking strategies, internal protocols) belongs inside the function or struct, not in its signature or public fields.

Signs of leaking complexity:
- Callers must pass internal state or flags
- Public fields hold mutex guards or intermediate results
- A function's signature reveals its implementation strategy

Design from the outside in: write the ideal call site first, then implement to match it.

## 6. Error Handling — Use `anyhow::Result`

For application code and library internals where the caller does not need to match on error variants, use `anyhow::Result<T>`. Never use `Result<T, ()>` or `Result<T, String>`.

```rust
use anyhow::{Context, Result};

fn load_config(path: &Path) -> Result<Config> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(toml::from_str(&raw)?)
}
```

For library public APIs where callers need to distinguish error kinds, define a typed error with `thiserror` (see rule 7) and keep `anyhow` to internals.

## 7. Custom Error Types — Use `thiserror`

When a typed error is necessary (public library API, error matching), derive it with `thiserror`. Never implement `std::error::Error` by hand.

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found at {path}")]
    NotFound { path: PathBuf },
    #[error("invalid value for field `{field}`: {source}")]
    InvalidField { field: String, #[source] source: anyhow::Error },
}
```

## 8. Derive Formatting — Use `derive_more` Before Hand-Rolling

When the standard `#[derive(Debug)]` or `#[derive(Display)]` macros are insufficient, reach for `derive_more` before writing an impl manually.

```rust
use derive_more::{Display, Debug};

#[derive(Display, Debug)]
#[display("Peer({addr})")]
pub struct PeerInfo {
    addr: SocketAddr,
}
```

Only write a manual `impl fmt::Display` or `impl fmt::Debug` if `derive_more` cannot express the required format.

## 9. CLI Argument Parsing — Use `clap`

Parse all command-line arguments with `clap` using its derive API. Do not use `std::env::args()`, `getopts`, or ad-hoc string splitting.

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "mytool", version, about)]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    #[arg(short, long)]
    verbose: bool,
}

fn main() {
    let args = Args::parse();
}
```

## 10. Resource Lifecycle — RAII

Manage resources with RAII: the object is both the resource and its handler.

- Object owns the resource, the lifecycle, and the operations — do not split them
- `Drop` signals cancellation; `async fn close()` cancels and drains
- `tokio::spawn` lives at the call site; the spawned function stays pure; the cancellation `select!` sits at the top of the spawned task

## 11. Cryptography — Use `aws-lc-rs`

For all cryptographic operations, depend on `aws-lc-rs`. Do not use `ring` or the RustCrypto family (`aes`, `sha2`, `rsa`, etc.) as direct dependencies.

```toml
[dependencies]
aws-lc-rs = "1"
```

`aws-lc-rs` is API-compatible with `ring` for most use cases and is backed by AWS's maintained fork of BoringSSL, with FIPS support available.

## 12. Scoped Cleanup — Use `scopeguard::defer!`

When a local variable or side effect needs guaranteed cleanup at scope exit — but doesn't warrant a full RAII wrapper type — use `scopeguard::defer!`.

```rust
use scopeguard::defer;

fn with_temp_file() -> Result<()> {
    let path = create_temp_file()?;
    defer! {
        let _ = fs::remove_file(&path);
    }

    process(&path)?;
    Ok(())
}
```

Use `defer!` for: temporary files, unlocking external resources, resetting global state in tests, and any other ad-hoc cleanup that runs once. Prefer a proper `Drop` impl when the same pattern recurs across multiple call sites.

## 13. `Arc<Self>` — Constructors That Need Weak Back-References

When a child/sub-object needs to hold a reference back to its parent, the constructor must return `Arc<Self>` (not `Self`). This lets the constructor store a `Weak<Self>` internally without a separate `Arc::new` call at the call site.

```rust
// ✅ correct
impl Foo {
    pub fn new(…) -> Arc<Self> {
        Arc::new(Self { … })
    }
}

// ❌ avoid — callers must remember to wrap, and internal Weak cannot be set up
impl Foo {
    pub fn new(…) -> Self { … }
}
```

## 14. Mutex Poisoning — Always Panic

A poisoned `Mutex` means a thread panicked while holding the lock — the protected data may be in an inconsistent state. Always propagate the panic:

```rust
// ✅
let guard = mutex.lock().expect("Mutex poisoned");

// ❌ silently recovers from a potentially corrupted state
let guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
```

## 15. Channels — Prefer Bounded; Prefer Function Calls Over Channels

Unbounded channels (`std::sync::mpsc::channel()`, `tokio::sync::mpsc::unbounded_channel()`) have no backpressure. A fast producer can cause unbounded memory growth without the sender ever knowing. Prefer bounded channels. If you genuinely need an unbounded channel, document the reasoning.

Prefer function-call / direct-method-call data flow over channel-based data passing. Channels are appropriate for *events* and *commands*, not for *query responses* that could be a return value.

## 16. Lock Scope — Minimise `MutexGuard` Lifetime

Keep `MutexGuard` lifetimes as short as possible. Place the lock acquisition and all operations on the guard inside a dedicated block:

```rust
// ✅ guard released when block exits
{
    let mut data = self.inner.lock().expect("poisoned");
    data.count += 1;
} // ← guard dropped here

do_something_without_holding_lock();

// ❌ guard held for the rest of the enclosing scope
let mut data = self.inner.lock().expect("poisoned");
data.count += 1;
do_something_without_holding_lock(); // lock still held!
```

Never call `drop(guard)` on a `MutexGuard`. Use a block scope instead — explicit drops are easy to miss when refactoring.

## Review Actions

For each Rust-specific violation, show the offending snippet and a corrected replacement that follows the rule above.
