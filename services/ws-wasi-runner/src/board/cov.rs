//! The `/cov` directory a guest built for coverage writes its profile into as its `run` ends.
//!
//! An instrumented guest imports `wasi:filesystem` for that one write, and could not instantiate on a board without
//! it. What it writes is discarded: the host runner runs the same guests and collects their profiles, so the board's
//! run would only replace one. The guest's libc imports more of a descriptor than the write needs, and those
//! functions answer `unsupported`. Each is defined dynamically, so that the guest's types describe their values
//! rather than a copy of every flag and error code.

use wasmtime::component::types::ComponentFunc;
use wasmtime::component::{Linker, Resource, ResourceType, Val};

use super::host::BoardHost;
use super::platform::{Platform, Transport};
use super::wasi::{Ctx, OutputStream, PREVIEW2, drop_from_table};

/// A `wasi:filesystem` descriptor: `/cov`, or a file the guest opened in it.
#[derive(Clone, Copy)]
enum Descriptor {
    /// `/cov` itself.
    Directory,
    /// A file in `/cov`.
    File,
}

/// Define `wasi:filesystem`'s `types` and `preopens`, with `/cov` the only directory.
///
/// # Errors
///
/// When the linker already holds one of them.
#[expect(clippy::single_call_fn, reason = "the coverage step of linking WASI")]
pub(super) fn add_to_linker<P, T>(linker: &mut Linker<BoardHost<P, T>>) -> wasmtime::Result<()>
where
    P: Platform + 'static,
    T: Transport + 'static,
{
    let mut types = linker.instance(&format!("wasi:filesystem/types@{PREVIEW2}"))?;
    types.resource(
        "descriptor",
        ResourceType::host::<Descriptor>(),
        drop_from_table::<Descriptor, P, T>,
    )?;
    types.func_new(
        "[method]descriptor.open-at",
        |mut store: Ctx<'_, P, T>, _: ComponentFunc, params: &[Val], results: &mut [Val]| {
            let outcome = match descriptor(&mut store, params)? {
                Descriptor::Directory => {
                    let file = store.data_mut().table().push(Descriptor::File)?;
                    Ok(Val::Resource(file.try_into_resource_any(&mut store)?))
                }
                Descriptor::File => Err("not-directory"),
            };
            answer(results, outcome)
        },
    )?;
    types.func_new(
        "[method]descriptor.write-via-stream",
        |mut store: Ctx<'_, P, T>, _: ComponentFunc, params: &[Val], results: &mut [Val]| {
            let outcome = match descriptor(&mut store, params)? {
                Descriptor::File => {
                    let stream = store.data_mut().table().push(OutputStream::Discard)?;
                    Ok(Val::Resource(stream.try_into_resource_any(&mut store)?))
                }
                Descriptor::Directory => Err("is-directory"),
            };
            answer(results, outcome)
        },
    )?;
    for method in [
        "read-via-stream",
        "append-via-stream",
        "get-flags",
        "stat",
        "metadata-hash",
    ] {
        types.func_new(
            &format!("[method]descriptor.{method}"),
            |_: Ctx<'_, P, T>, _: ComponentFunc, _: &[Val], results: &mut [Val]| answer(results, Err("unsupported")),
        )?;
    }

    linker
        .instance(&format!("wasi:filesystem/preopens@{PREVIEW2}"))?
        .func_wrap("get-directories", |mut store: Ctx<'_, P, T>, (): ()| {
            let directory = store.data_mut().table().push(Descriptor::Directory)?;
            Ok((vec![(directory, "/cov".to_owned())],))
        })
}

/// The descriptor a `[method]descriptor.*` call was made on, its first parameter.
fn descriptor<P, T>(store: &mut Ctx<'_, P, T>, params: &[Val]) -> wasmtime::Result<Descriptor>
where
    P: 'static,
    T: 'static,
{
    let Some(Val::Resource(this)) = params.first() else {
        return Err(wasmtime::Error::msg(
            "a descriptor method was called without its descriptor",
        ));
    };
    let this: Resource<Descriptor> = this.try_into_resource(&mut *store)?;
    Ok(*store.data_mut().table().get(&this)?)
}

/// Return `outcome` as the `result<_, error-code>` every descriptor function returns, its error named by its case.
fn answer(results: &mut [Val], outcome: Result<Val, &str>) -> wasmtime::Result<()> {
    let Some(result) = results.first_mut() else {
        return Err(wasmtime::Error::msg(
            "a descriptor function was called without room for its result",
        ));
    };
    *result = Val::Result(match outcome {
        Ok(value) => Ok(Some(Box::new(value))),
        Err(code) => Err(Some(Box::new(Val::Enum(code.to_owned())))),
    });
    Ok(())
}
