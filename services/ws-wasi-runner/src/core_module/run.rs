//! Decode, link and run a core module, driving the interpreter between the imports that pause it.

#![expect(
    clippy::future_not_send,
    reason = "SpaceWasm's engine is neither Send nor Sync, so no future driving it can be; a host gives it a thread"
)]

use core::pin::Pin;
use core::task::{Context, Poll};

use et_core_guest::abi;
use spacewasm::{
    CodeBuilder, CompilerOptions, Engine, ExportDesc, Interpreter, InterpreterResult, InterpreterRunner as _, Module,
    Ref, ValType, WasmRef,
};

use super::allocator::LinearMemory;
use super::imports::{self, Shared};
use super::stream::ModuleStream;
use super::{CoreError, CoreHost};

/// How many instructions the interpreter runs before yielding to the executor.
const FUEL_SLICE: usize = 100_000;
/// The engine's operand and call stack, in words.
const STACK_WORDS: usize = 0x10_0000;
/// The deepest nesting of blocks the decoder validates a function with.
const MAX_CONTROL_FRAMES: usize = 256;
/// The deepest operand stack the decoder validates a function with.
const MAX_STACK_DEPTH: usize = 1024;
/// The most pages of compiled code a module may compile to.
const MAX_CODE_PAGES: usize = 0x1_0000;
/// What the store names the guest's module.
const MODULE_NAME: &str = "main";

/// Run the core module `wasm` until its `run` returns, carrying its imports out on `host`.
///
/// The module's start function, when it has one, runs first. Between imports the interpreter runs in slices of
/// instructions, yielding to the executor after each.
///
/// # Errors
///
/// When the module does not decode, validate or link, has no `run` of the right type, traps, or its `run` returns
/// anything but [`abi::SUCCESS`].
pub async fn run<H>(wasm: &[u8], mut host: H) -> Result<(), CoreError>
where
    H: CoreHost,
{
    let shared = Shared::default();
    let mut code = CodeBuilder::new(CompilerOptions {
        allow_memory_grow: true,
        max_backpatch_iterations: None,
        max_code_pages: MAX_CODE_PAGES,
    })?;
    let mut engine = Engine::new(STACK_WORDS, 1, spacewasm::Vec::from_array([imports::module(&shared)?])?)?;
    let memory = spacewasm::Rc::new(LinearMemory)?.into_wasm_memory_allocator();
    let module = Module::new::<MAX_CONTROL_FRAMES, MAX_STACK_DEPTH>(
        MODULE_NAME,
        &mut ModuleStream::new(wasm),
        &mut engine.store,
        &mut code,
        memory,
    )?;
    let entry = entry(&module)?;
    let module_ref = engine.push_module(module)?;
    if let Some(start) = engine.module_start(module_ref) {
        engine.invoke(start, &[])?;
        drive(&mut engine, &code, &shared, &mut host).await?;
    }
    engine.invoke(
        WasmRef {
            module: module_ref,
            index: entry,
        },
        &[],
    )?;
    drive(&mut engine, &code, &shared, &mut host).await?;
    match engine.result.map(|result| result.read_i32()) {
        Some(abi::SUCCESS) => Ok(()),
        Some(failure) => Err(CoreError::Guest(failure)),
        None => Err(CoreError::NoResult),
    }
}

/// The index of `module`'s own `run` function, checking it takes nothing and returns an `i32`.
#[expect(
    clippy::single_call_fn,
    reason = "the export lookup of run, named apart from the decoding around it"
)]
fn entry(module: &Module) -> Result<u16, CoreError> {
    let Some(ExportDesc::Func(index)) = module
        .exports
        .iter()
        .find(|export| export.name == abi::RUN)
        .map(|export| &export.desc)
    else {
        return Err(CoreError::NoRun);
    };
    let Some(Ref::Module(local)) = module.get_func_ref(*index) else {
        return Err(CoreError::NoRun);
    };
    let signature = module
        .functions
        .get(usize::from(local))
        .and_then(|function| module.types.get(usize::try_from(function.ty.0).ok()?));
    match signature {
        Some(signature) if signature.params.is_empty() && *signature.returns == [ValType::I32] => Ok(local),
        _ => Err(CoreError::RunSignature),
    }
}

/// Run the function the engine was last invoked with to its end, carrying out each import that pauses it.
async fn drive<H>(engine: &mut Engine, code: &CodeBuilder, shared: &Shared, host: &mut H) -> Result<(), CoreError>
where
    H: CoreHost,
{
    loop {
        match Interpreter.run(code.pages(), engine, FUEL_SLICE) {
            InterpreterResult::Finished => return Ok(()),
            InterpreterResult::Trap(reason) => return Err(CoreError::Trap(reason)),
            InterpreterResult::OutOfFuel => YieldNow::default().await,
            InterpreterResult::Pause => imports::resume(host, shared, engine).await?,
        }
    }
}

/// Yields to the executor once, so that whatever else it runs makes progress between slices of the guest.
#[derive(Default)]
struct YieldNow {
    /// Whether this future has yielded already.
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    /// Pending the first time, waking the task at once, and ready the second.
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
