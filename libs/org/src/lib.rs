//! Who this project is, as the strings that carry its identity into registries, images and manifests.
//!
//! Everything here is one of two facts -- the organisation the project publishes under, and the prefix its
//! own crates and modules carry -- or a name built from one of them. They are collected into a crate of
//! their own because they are the same fact wearing different clothes: the npm scope, the container
//! registry, the repository URL and the crate prefix all move together if the project is ever renamed, and
//! a copy of any of them living somewhere else is a copy that gets missed. `org-identity-lives-in-et-org`
//! keeps them here, so a new spelling has to be built from these rather than written out again.
//!
//! The organisation name itself is written exactly once, in [`ORG`]'s macro, and every longer string is
//! assembled from it. That is what makes the rule enforceable rather than aspirational: there is a single
//! literal to protect.

/// The organisation name as a literal, for the constants below to build on.
///
/// A macro rather than a `const` because `concat!` joins literals at compile time and cannot take one: a
/// `const` would have to be spelled out again in every string that embeds it, which is the duplication this
/// crate exists to prevent.
#[macro_export]
macro_rules! org {
    () => {
        "edge-toolkit"
    };
}

/// The organisation this project publishes under.
pub const ORG: &str = org!();

/// The prefix every crate and served module of this project's own carries.
///
/// What separates a module of ours from a third-party one in a dependency list: ours is published under the
/// owner scope and has to be named as the registry knows it, whereas somebody else's is already published
/// under exactly the name it is declared by.
pub const CRATE_PREFIX: &str = "et-";

/// The npm scope as a literal, for a `concat!` that has to embed it.
///
/// [`NPM_SCOPE`] is the constant to prefer. This exists because `concat!` takes literals and not constants,
/// so a module building one of its own URLs at compile time -- `/modules/<scope><module>/...` -- cannot use
/// one, and writing the scope out at that call site is what this crate exists to stop.
#[macro_export]
macro_rules! npm_scope {
    () => {
        concat!("@", $crate::org!(), "/")
    };
}

/// The npm scope this project's packages are published under, including the trailing separator.
///
/// GitHub Packages accepts nothing else -- a package not scoped to the repository owner is rejected -- and
/// the name it accepts is the name the hub serves the module under, so it reaches URLs too.
pub const NPM_SCOPE: &str = npm_scope!();

/// This repository's URL as a literal, for a `concat!` that has to embed it.
///
/// [`REPOSITORY_URL`] is the constant to prefer. This exists because `concat!` takes literals and not
/// constants, so a generated file that builds a line around the URL at compile time cannot use one -- and
/// writing the URL out at that call site is exactly what this crate exists to stop.
#[macro_export]
macro_rules! repository_url {
    () => {
        concat!("https://github.com/", $crate::org!(), "/core")
    };
}

/// This repository, as the URL that identifies it to anything reading a published artifact.
pub const REPOSITORY_URL: &str = repository_url!();

/// The container registry this project's own images are published under.
pub const IMAGE_REGISTRY: &str = concat!("ghcr.io/", org!(), "/core");
