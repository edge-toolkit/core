//! Why a core module did not run to completion.

/// Why a core module did not run to completion.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CoreError {
    /// `SpaceWasm` could not allocate what it needed.
    #[error("SpaceWasm could not allocate: {0:?}")]
    Alloc(spacewasm::AllocError),
    /// `SpaceWasm` could not set up its engine.
    #[error("SpaceWasm could not set up its engine: {0:?}")]
    Engine(spacewasm::MemoryError),
    /// The `et` imports could not be named.
    #[error("the `et` imports could not be named: {0:?}")]
    ImportName(spacewasm::HostNameError),
    /// The `et` imports could not be declared.
    #[error("the `et` imports could not be declared: {0:?}")]
    Import(spacewasm::HostFunctionError),
    /// The module is not Wasm 1.0 `SpaceWasm` accepts, or imports what the `et` module does not provide.
    #[error("the module did not decode, validate or link: {0:?}")]
    Decode(spacewasm::ParseError),
    /// The module exports no `run` function of its own.
    #[error("the module exports no `run` function of its own")]
    NoRun,
    /// The module's `run` does not take nothing and return an `i32`.
    #[error("the module's `run` must take nothing and return an i32")]
    RunSignature,
    /// The engine could not start a function.
    #[error("the engine could not start the guest: {0:?}")]
    Invoke(spacewasm::InvokeError),
    /// The engine could not resume the guest after an import.
    #[error("the engine could not resume the guest: {0:?}")]
    Resume(spacewasm::ResumeError),
    /// The guest trapped.
    #[error("the guest trapped: {0:?}")]
    Trap(spacewasm::TrapReason),
    /// The guest's `run` returned a failure, after logging why.
    #[error("the guest's run returned {0}")]
    Guest(i32),
    /// The guest's `run` finished without a result.
    #[error("the guest's run returned no result")]
    NoResult,
}

impl From<spacewasm::AllocError> for CoreError {
    fn from(error: spacewasm::AllocError) -> Self {
        Self::Alloc(error)
    }
}

impl From<spacewasm::MemoryError> for CoreError {
    fn from(error: spacewasm::MemoryError) -> Self {
        Self::Engine(error)
    }
}

impl From<spacewasm::HostNameError> for CoreError {
    fn from(error: spacewasm::HostNameError) -> Self {
        Self::ImportName(error)
    }
}

impl From<spacewasm::HostFunctionError> for CoreError {
    fn from(error: spacewasm::HostFunctionError) -> Self {
        Self::Import(error)
    }
}

impl From<spacewasm::ParseError> for CoreError {
    fn from(error: spacewasm::ParseError) -> Self {
        Self::Decode(error)
    }
}

impl From<spacewasm::InvokeError> for CoreError {
    fn from(error: spacewasm::InvokeError) -> Self {
        Self::Invoke(error)
    }
}

impl From<spacewasm::ResumeError> for CoreError {
    fn from(error: spacewasm::ResumeError) -> Self {
        Self::Resume(error)
    }
}
