//! ws-module packages built into the binary, served through picoserve where the `std` server serves them from disk.
//!
//! A board has no filesystem to scan, so its embedder hands [`serve`](crate::picoserve_server::serve) a [`Modules`]
//! whose every file is a `&'static [u8]`, usually through `include_bytes!` in generated code. Each answers a matching
//! `If-None-Match` with a 304, so a browser reloading the page does not fetch its wasm from the board again. The routes
//! are the `std` server's: `GET /modules/` lists the module names, `GET /modules/{name}/{path}` is one file of one
//! module, and the root module's files are served from `/`, `index.html` answering `/` itself.
//!
//! A package too big for the board's flash can be listed as served [`elsewhere`](Modules::elsewhere) instead: a request
//! for one of its files is redirected to the same path under [`elsewhere_url`](Modules::elsewhere_url), such as a CDN
//! that mirrors the npm registry, and its name is listed with the rest so a page cannot tell the difference.

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use picoserve::ResponseSent;
use picoserve::io::{Read, Write};
use picoserve::request::{Path, Request};
use picoserve::response::{Content, IntoResponse as _, Json, Response, ResponseWriter, StatusCode};
use picoserve::routing::{MethodNotAllowed, NotFound, PathRouter as _, PathRouterService, RequestHandler as _};
use picoserve::url_encoded::UrlEncodedString;

/// Where module files are served under, as the `std` server's modules service mounts them.
const MODULES_PREFIX: &str = "/modules";
/// The file that answers a request for the root module's `/`.
const INDEX_FILE: &str = "index.html";

/// One file of a module, with what a browser needs to cache it.
///
/// Its entity tag is worked out when the file is built in rather than here: hashing a module's wasm as a `const` would
/// cost the compiler minutes, and the board cycles it would cost at run time.
#[derive(Debug)]
#[non_exhaustive]
pub struct ModuleFile {
    /// The `Content-Type` the file is served as.
    pub content_type: &'static str,
    /// The file's bytes.
    pub body: &'static [u8],
    /// A tag that changes whenever `body` does, sent unquoted as the file's `ETag`.
    pub etag: &'static str,
}

impl ModuleFile {
    /// The file `body`, served as `content_type` and tagged `etag`.
    #[must_use]
    pub const fn new(content_type: &'static str, body: &'static [u8], etag: &'static str) -> Self {
        Self {
            content_type,
            body,
            etag,
        }
    }

    /// Whether an `If-None-Match` header of `tags` already names this file, so a 304 answers it.
    fn is_matched_by(&self, tags: &[u8]) -> bool {
        tags.split(|byte| *byte == b',').any(|tag| {
            let tag = tag.trim_ascii();
            let tag = tag.strip_prefix(b"W/").unwrap_or(tag);
            tag == b"*" || tag.strip_prefix(b"\"").and_then(|tag| tag.strip_suffix(b"\"")) == Some(self.etag.as_bytes())
        })
    }
}

impl Content for &ModuleFile {
    fn content_type(&self) -> &'static str {
        self.content_type
    }

    fn content_length(&self) -> usize {
        self.body.len()
    }

    async fn write_content<W>(self, mut writer: W) -> Result<(), W::Error>
    where
        W: Write,
    {
        writer.write_all(self.body).await
    }
}

/// One module's files, each by its path within the module, such as `pkg.js` or `assets/model.bin`.
#[derive(Debug)]
#[non_exhaustive]
pub struct Module {
    /// The module's name, exactly as its `package.json` declares it, scope and all.
    pub name: &'static str,
    /// Every file of the module, by its path relative to the module's root, without a leading `/`.
    pub files: &'static [(&'static str, ModuleFile)],
}

impl Module {
    /// The module `name`, made of `files`.
    #[must_use]
    pub const fn new(name: &'static str, files: &'static [(&'static str, ModuleFile)]) -> Self {
        Self { name, files }
    }

    /// The file `path` names within this module, a request path relative to the module's root with its leading `/`.
    ///
    /// The module's own root is its `index.html` only when `indexed`, as only the root module's is.
    fn file(&self, path: UrlEncodedString<'_>, indexed: bool) -> Target<'_> {
        // Both callers hand over a path that starts with its `/`, so there is always one to take off.
        let relative = path.strip_prefix("/").unwrap_or(path);
        let relative = if indexed && relative.is_empty() {
            UrlEncodedString(INDEX_FILE)
        } else {
            relative
        };
        self.files
            .iter()
            .find(|(name, _)| relative == *name)
            .map_or(Target::Missing, |(_, file)| Target::File(file))
    }
}

/// Every module a board serves: those built into it, and those it redirects to [`elsewhere_url`](Self::elsewhere_url).
#[derive(Debug)]
#[non_exhaustive]
pub struct Modules {
    /// The modules built into the binary.
    pub embedded: &'static [Module],
    /// The name of the embedded module served at `/`, or empty to serve nothing there, as `MODULES_ROOT` does.
    pub root: &'static str,
    /// The names of the modules served from [`elsewhere_url`](Self::elsewhere_url) rather than from the binary.
    pub elsewhere: &'static [&'static str],
    /// The base a request for a module in [`elsewhere`](Self::elsewhere) is redirected to, its path appended as is.
    pub elsewhere_url: &'static str,
}

impl Modules {
    /// No modules at all: `/modules/` lists none and every other module path is not found.
    pub const NONE: Self = Self::new(&[], "", &[], "");

    /// The modules `embedded`, `root` among them, with `elsewhere` redirected to `elsewhere_url`.
    #[must_use]
    pub const fn new(
        embedded: &'static [Module],
        root: &'static str,
        elsewhere: &'static [&'static str],
        elsewhere_url: &'static str,
    ) -> Self {
        Self {
            embedded,
            root,
            elsewhere,
            elsewhere_url,
        }
    }

    /// Every module name, embedded or not, sorted as the `std` server lists them.
    fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.embedded.iter().map(|module| module.name).collect();
        names.extend_from_slice(self.elsewhere);
        names.sort_unstable();
        names
    }

    /// The embedded file `path` names, a path under `/modules` that starts with its module's name.
    fn embedded_file(&self, path: UrlEncodedString<'_>) -> Target<'_> {
        let Some(path) = path.strip_prefix("/") else {
            return Target::Missing;
        };
        for module in self.embedded {
            if let Some(rest) = path.strip_prefix(module.name)
                && rest.0.starts_with('/')
            {
                return module.file(rest, false);
            }
        }
        Target::Missing
    }

    /// Whether `path` names a file of a module served elsewhere.
    fn is_elsewhere(&self, path: UrlEncodedString<'_>) -> bool {
        let Some(path) = path.strip_prefix("/") else {
            return false;
        };
        !self.elsewhere_url.is_empty()
            && self
                .elsewhere
                .iter()
                .any(|name| path.strip_prefix(name).is_some_and(|rest| rest.0.starts_with('/')))
    }
}

/// The routes [`Modules`] answers, as the fallback of a server's other routes: a request none of them matched.
pub(crate) struct ModuleRoutes<'modules>(&'modules Modules);

impl<'modules> ModuleRoutes<'modules> {
    /// The routes `modules` answers.
    #[expect(
        clippy::single_call_fn,
        reason = "the constructor that keeps the field private to this module, for the one server that mounts it"
    )]
    pub(crate) const fn new(modules: &'modules Modules) -> Self {
        Self(modules)
    }
}

impl<State, CurrentPathParameters> PathRouterService<State, CurrentPathParameters> for ModuleRoutes<'_> {
    #[expect(
        clippy::future_not_send,
        reason = "picoserve's futures hold Cells; its executors run each connection on one thread"
    )]
    async fn call_path_router_service<R, W>(
        &self,
        state: &State,
        current_path_parameters: CurrentPathParameters,
        path: Path<'_>,
        request: Request<'_, R>,
        response_writer: W,
    ) -> Result<ResponseSent, W::Error>
    where
        R: Read,
        W: ResponseWriter<Error = R::Error>,
    {
        let modules = self.0;
        let target = Target::named_by(modules, UrlEncodedString(path.encoded()));
        if !matches!(target, Target::Missing) && !request.parts.method().eq_ignore_ascii_case("get") {
            return MethodNotAllowed
                .call_request_handler(state, current_path_parameters, request, response_writer)
                .await;
        }
        match target {
            Target::Missing => {
                NotFound
                    .call_path_router(state, current_path_parameters, path, request, response_writer)
                    .await
            }
            Target::List => {
                Json(modules.names())
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
            Target::File(file) => {
                let etag = format_args!("\"{}\"", file.etag);
                let cached = request
                    .parts
                    .headers()
                    .get("If-None-Match")
                    .is_some_and(|tags| file.is_matched_by(tags.as_raw()));
                let connection = request.body_connection.finalize().await?;
                if cached {
                    Response::new(StatusCode::NOT_MODIFIED, "")
                        .with_header("ETag", etag)
                        .write_to(connection, response_writer)
                        .await
                } else {
                    Response::ok(file)
                        .with_header("ETag", etag)
                        .write_to(connection, response_writer)
                        .await
                }
            }
            Target::Elsewhere(module_path) => {
                Response::new(StatusCode::TEMPORARY_REDIRECT, "")
                    .with_header("Location", format_args!("{}{module_path}", modules.elsewhere_url))
                    .write_to(request.body_connection.finalize().await?, response_writer)
                    .await
            }
        }
    }
}

/// What a request path names among [`Modules`].
enum Target<'request> {
    /// `/modules/`, the list of module names.
    List,
    /// One embedded file.
    File(&'request ModuleFile),
    /// A file of a module served elsewhere, by its path under `/modules`.
    Elsewhere(&'request str),
    /// Nothing these modules serve.
    Missing,
}

impl<'request> Target<'request> {
    /// What `path`, a whole request path, names among `modules`.
    #[expect(
        clippy::single_call_fn,
        reason = "the lookup step of ModuleRoutes, kept apart from writing what it finds"
    )]
    fn named_by(modules: &'request Modules, path: UrlEncodedString<'request>) -> Self {
        let Some(module_path) = path.strip_prefix(MODULES_PREFIX) else {
            return Self::root_file(modules, path);
        };
        if module_path.is_empty() || module_path.0 == "/" {
            return Self::List;
        }
        let file = modules.embedded_file(module_path);
        if matches!(file, Self::File(_)) {
            return file;
        }
        if modules.is_elsewhere(module_path) {
            return Self::Elsewhere(module_path.0);
        }
        // Not a module path after all, such as `/modulesfoo`, which the root module may still hold.
        if module_path.0.starts_with('/') {
            Self::Missing
        } else {
            Self::root_file(modules, path)
        }
    }

    /// The root module's file at `path`.
    fn root_file(modules: &'request Modules, path: UrlEncodedString<'request>) -> Self {
        modules
            .embedded
            .iter()
            .find(|module| !modules.root.is_empty() && module.name == modules.root)
            .map_or(Self::Missing, |root| root.file(path, true))
    }
}
