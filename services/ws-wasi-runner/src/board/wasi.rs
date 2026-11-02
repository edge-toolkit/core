//! The WASI Preview 2 a board hosts: what a guest built for `wasm32-wasip2` imports even when it never uses it, defined
//! by hand, since `wasmtime-wasi` needs `std`.
//!
//! Every ws-module guest imports the same few functions: its standard streams and environment, `exit`, the terminal
//! queries, and the monotonic clock and polling it sleeps with. A component imports only the functions it calls, so
//! these are all a guest needs; one importing any other fails to instantiate, naming it. Its standard streams go to the
//! board's console, its environment is empty, it has no terminal, and a pollable is ready at once or at a deadline on
//! the board's clock.
#![expect(
    clippy::integer_division_remainder_used,
    reason = "wasmtime's ComponentType derive, on StreamError, aligns with `%` in the impls it generates"
)]

#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, format, string::String, vec::Vec};

pub(super) use edge_toolkit::board::PREVIEW2;
use wasmtime::StoreContextMut;
use wasmtime::component::{ComponentType, Lift, Linker, Lower, Resource, ResourceType};

use super::host::BoardHost;
use super::platform::{Console, Platform, Transport};

/// How many bytes `check-write` lets the guest write at once; the board's console takes any amount.
const WRITE_BUDGET: u64 = 4096;

/// A `wasi:io/poll` pollable: ready now, or at a deadline on the board's clock.
#[derive(Clone, Copy, Debug)]
pub(super) enum Pollable {
    /// Ready already, as an output stream always is.
    Ready,
    /// Ready once [`Platform::now`] reaches this.
    Due(u64),
}

/// A `wasi:io/streams` output stream: where what the guest writes to it goes.
#[derive(Clone, Copy)]
pub(super) enum OutputStream {
    /// One of the board's console streams.
    Console(Console),
    /// Nowhere: a file in the coverage build's `/cov`.
    #[cfg(feature = "coverage")]
    Discard,
}

/// A `wasi:io/streams` input stream, the guest's stdin, which never has anything to read.
pub(super) struct InputStream;

/// A `wasi:io/error`, which a board never raises.
pub(super) enum IoError {}

/// A `wasi:cli/terminal-input`, which a board never has.
pub(super) enum TerminalInput {}

/// A `wasi:cli/terminal-output`, which a board never has.
pub(super) enum TerminalOutput {}

/// `wasi:io/streams`' `stream-error`, which a board's console never fails with.
#[derive(ComponentType, Lift, Lower)]
#[component(variant)]
enum StreamError {
    #[component(name = "last-operation-failed")]
    LastOperationFailed(Resource<IoError>),
    #[component(name = "closed")]
    Closed,
}

/// The store a host function runs against.
pub(super) type Ctx<'store, P, T> = StoreContextMut<'store, BoardHost<P, T>>;

/// Define the WASI Preview 2 functions and resources every ws-module guest imports.
///
/// # Errors
///
/// When the linker already holds one of them.
#[expect(
    clippy::single_call_fn,
    reason = "the WASI step of linking, apart from the runner world's"
)]
pub(super) fn add_to_linker<P, T>(linker: &mut Linker<BoardHost<P, T>>) -> wasmtime::Result<()>
where
    P: Platform + 'static,
    T: Transport + 'static,
{
    add_poll(linker)?;
    add_streams(linker)?;
    add_cli(linker)?;
    #[cfg(feature = "coverage")]
    super::cov::add_to_linker(linker)?;
    Ok(())
}

/// `wasi:io/poll`, and the monotonic clock the guest's pollables come from.
#[expect(clippy::single_call_fn, reason = "the polling part of add_to_linker")]
fn add_poll<P, T>(linker: &mut Linker<BoardHost<P, T>>) -> wasmtime::Result<()>
where
    P: Platform + 'static,
    T: Transport + 'static,
{
    let mut poll = linker.instance(&format!("wasi:io/poll@{PREVIEW2}"))?;
    poll.resource(
        "pollable",
        ResourceType::host::<Pollable>(),
        drop_from_table::<Pollable, P, T>,
    )?;
    poll.func_wrap_async(
        "[method]pollable.block",
        |mut store: Ctx<'_, P, T>, (pollable,): (Resource<Pollable>,)| {
            Box::new(async move {
                let host = store.data_mut();
                let pollable = *host.table().get(&pollable)?;
                let _ready = ready(host.platform(), &[pollable]).await;
                Ok(())
            })
        },
    )?;
    poll.func_wrap_async(
        "poll",
        |mut store: Ctx<'_, P, T>, (pollables,): (Vec<Resource<Pollable>>,)| {
            Box::new(async move {
                let host = store.data_mut();
                let mut waiting = Vec::with_capacity(pollables.len());
                for pollable in &pollables {
                    waiting.push(*host.table().get(pollable)?);
                }
                Ok((ready(host.platform(), &waiting).await,))
            })
        },
    )?;

    linker
        .instance(&format!("wasi:clocks/monotonic-clock@{PREVIEW2}"))?
        .func_wrap("subscribe-duration", |mut store: Ctx<'_, P, T>, (duration,): (u64,)| {
            let host = store.data_mut();
            let deadline = host.platform().now().saturating_add(duration);
            Ok((host.table().push(Pollable::Due(deadline))?,))
        })
}

/// `wasi:io/streams` and its errors: the guest's standard streams, written to the board's console.
#[expect(clippy::single_call_fn, reason = "the streams part of add_to_linker")]
fn add_streams<P, T>(linker: &mut Linker<BoardHost<P, T>>) -> wasmtime::Result<()>
where
    P: Platform + 'static,
    T: Transport + 'static,
{
    linker.instance(&format!("wasi:io/error@{PREVIEW2}"))?.resource(
        "error",
        ResourceType::host::<IoError>(),
        |_, _| Ok(()),
    )?;

    let mut streams = linker.instance(&format!("wasi:io/streams@{PREVIEW2}"))?;
    streams.resource(
        "input-stream",
        ResourceType::host::<InputStream>(),
        drop_from_table::<InputStream, P, T>,
    )?;
    streams.resource(
        "output-stream",
        ResourceType::host::<OutputStream>(),
        drop_from_table::<OutputStream, P, T>,
    )?;
    streams.func_wrap(
        "[method]output-stream.check-write",
        |_: Ctx<'_, P, T>, (_stream,): (Resource<OutputStream>,)| Ok((Ok::<u64, StreamError>(WRITE_BUDGET),)),
    )?;
    streams.func_wrap(
        "[method]output-stream.write",
        |mut store: Ctx<'_, P, T>, (stream, contents): (Resource<OutputStream>, Vec<u8>)| {
            let host = store.data_mut();
            match *host.table().get(&stream)? {
                OutputStream::Console(console) => host.platform().write(console, &contents),
                #[cfg(feature = "coverage")]
                OutputStream::Discard => {}
            }
            Ok((Ok::<(), StreamError>(()),))
        },
    )?;
    streams.func_wrap(
        "[method]output-stream.blocking-flush",
        |_: Ctx<'_, P, T>, (_stream,): (Resource<OutputStream>,)| Ok((Ok::<(), StreamError>(()),)),
    )?;
    streams.func_wrap(
        "[method]output-stream.subscribe",
        |mut store: Ctx<'_, P, T>, (_stream,): (Resource<OutputStream>,)| {
            Ok((store.data_mut().table().push(Pollable::Ready)?,))
        },
    )
}

/// `wasi:cli`: the standard streams, the environment, `exit`, and the terminal queries.
#[expect(clippy::single_call_fn, reason = "the cli part of add_to_linker")]
fn add_cli<P, T>(linker: &mut Linker<BoardHost<P, T>>) -> wasmtime::Result<()>
where
    P: Platform + 'static,
    T: Transport + 'static,
{
    for (interface, getter, console) in [
        ("stdout", "get-stdout", Console::Stdout),
        ("stderr", "get-stderr", Console::Stderr),
    ] {
        linker
            .instance(&format!("wasi:cli/{interface}@{PREVIEW2}"))?
            .func_wrap(getter, move |mut store: Ctx<'_, P, T>, (): ()| {
                Ok((store.data_mut().table().push(OutputStream::Console(console))?,))
            })?;
    }
    linker
        .instance(&format!("wasi:cli/stdin@{PREVIEW2}"))?
        .func_wrap("get-stdin", |mut store: Ctx<'_, P, T>, (): ()| {
            Ok((store.data_mut().table().push(InputStream)?,))
        })?;
    linker
        .instance(&format!("wasi:cli/environment@{PREVIEW2}"))?
        .func_wrap("get-environment", |_: Ctx<'_, P, T>, (): ()| {
            Ok((Vec::<(String, String)>::new(),))
        })?;
    linker.instance(&format!("wasi:cli/exit@{PREVIEW2}"))?.func_wrap(
        "exit",
        |_: Ctx<'_, P, T>, (status,): (Result<(), ()>,)| -> wasmtime::Result<()> {
            let outcome = if status.is_ok() { "success" } else { "failure" };
            Err(wasmtime::Error::msg(format!("the guest exited with {outcome}")))
        },
    )?;

    linker
        .instance(&format!("wasi:cli/terminal-input@{PREVIEW2}"))?
        .resource("terminal-input", ResourceType::host::<TerminalInput>(), |_, _| Ok(()))?;
    linker
        .instance(&format!("wasi:cli/terminal-output@{PREVIEW2}"))?
        .resource("terminal-output", ResourceType::host::<TerminalOutput>(), |_, _| Ok(()))?;
    linker
        .instance(&format!("wasi:cli/terminal-stdin@{PREVIEW2}"))?
        .func_wrap("get-terminal-stdin", |_: Ctx<'_, P, T>, (): ()| {
            Ok((None::<Resource<TerminalInput>>,))
        })?;
    for (interface, getter) in [
        ("terminal-stdout", "get-terminal-stdout"),
        ("terminal-stderr", "get-terminal-stderr"),
    ] {
        linker
            .instance(&format!("wasi:cli/{interface}@{PREVIEW2}"))?
            .func_wrap(getter, |_: Ctx<'_, P, T>, (): ()| {
                Ok((None::<Resource<TerminalOutput>>,))
            })?;
    }
    Ok(())
}

/// Drop the guest's handle to a `R` the board created, as a resource destructor.
pub(super) fn drop_from_table<R, P, T>(mut store: Ctx<'_, P, T>, rep: u32) -> wasmtime::Result<()>
where
    R: 'static,
{
    let _dropped: R = store.data_mut().table().delete(Resource::<R>::new_own(rep))?;
    Ok(())
}

/// The indices of `pollables` that are ready, waiting on `platform` until at least one is.
async fn ready<P>(platform: &P, pollables: &[Pollable]) -> Vec<u32>
where
    P: Platform,
{
    loop {
        let now = platform.now();
        let mut ready = Vec::new();
        let mut earliest = u64::MAX;
        for (index, pollable) in (0_u32..).zip(pollables) {
            match *pollable {
                Pollable::Ready => ready.push(index),
                Pollable::Due(deadline) if deadline <= now => ready.push(index),
                Pollable::Due(deadline) => earliest = earliest.min(deadline),
            }
        }
        // With nothing to wait on, nothing will ever be ready, so the guest is told so rather than left waiting.
        if !ready.is_empty() || earliest == u64::MAX {
            return ready;
        }
        platform.sleep_until(earliest).await;
    }
}
