//! The routes the server answers: the submission form, the endpoint it posts to, the registry, and any static files.
#![expect(
    clippy::single_call_fn,
    reason = "each route's handler is a named function, registered once by configure"
)]

use std::path::PathBuf;

use actix_web::error::{InternalError, JsonPayloadError};
use actix_web::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use actix_web::middleware::DefaultHeaders;
use actix_web::{HttpRequest, HttpResponse, web};
use serde_json::json;
use tracing::info;

use crate::registry::{RegistryTree, Submission};

/// The paste-and-register page served at `/submit`.
const SUBMIT_PAGE: &str = include_str!("submit.html");

/// The styles the server's pages share, served at `/probe.css`.
const STYLES: &str = include_str!("probe.css");

/// The module the server's pages post a submission with, served at `/register.js`.
const REGISTER_SCRIPT: &str = include_str!("register.mjs");

/// The module the probe page proposes its submissions on GitHub with where no server serves it, at `/propose.js`.
const PROPOSE_SCRIPT: &str = include_str!("propose.mjs");

/// The longest body `/register` or `/status` reads, past the largest submission the registry can hold.
///
/// A submission holds at most 256 results, each a name of at most 64 characters beside a value of at most 128, which
/// with its row and label is about 50 KB as JSON.
const MAX_BODY: usize = 64 * 1024;

/// Add the server's routes to `config`, adding to `registry` and serving the files under `root`, if given, at `/`.
///
/// Nothing is cached: the registry changes while signatures are being collected, and a probe page being worked on
/// changes between visits.
pub fn configure(config: &mut web::ServiceConfig, registry: web::Data<RegistryTree>, root: Option<PathBuf>) {
    let mut scope = web::scope("")
        .wrap(DefaultHeaders::new().add((CACHE_CONTROL, "no-store")))
        .app_data(registry)
        .route("/submit", embedded("text/html; charset=utf-8", SUBMIT_PAGE))
        .route("/probe.css", embedded("text/css; charset=utf-8", STYLES))
        .route(
            "/register.js",
            embedded("text/javascript; charset=utf-8", REGISTER_SCRIPT),
        )
        .route(
            "/propose.js",
            embedded("text/javascript; charset=utf-8", PROPOSE_SCRIPT),
        )
        .service(
            web::resource("/register")
                .app_data(json_body(submission_refused))
                .route(web::post().to(register)),
        )
        .service(
            web::resource("/status")
                .app_data(json_body(status_refused))
                .route(web::post().to(status)),
        )
        .route("/signatures.yaml", web::get().to(signatures));
    if let Some(root) = root {
        scope = scope.service(actix_files::Files::new("/", root).index_file("index.html"));
    }
    let _configured = config.service(scope);
}

/// A route answering a GET with `body`, a file built into the server, as `content_type`.
fn embedded(content_type: &'static str, body: &'static str) -> actix_web::Route {
    web::get().to(move || async move {
        HttpResponse::Ok()
            .insert_header((CONTENT_TYPE, content_type))
            .body(body)
    })
}

/// The registry as it stands, as one YAML document, for a probe page to say whether a device is in it already.
async fn signatures(registry: web::Data<RegistryTree>) -> HttpResponse {
    match registry.text() {
        Ok(text) => HttpResponse::Ok()
            .insert_header((CONTENT_TYPE, "application/yaml; charset=utf-8"))
            .body(text),
        Err(error) => HttpResponse::InternalServerError().body(error.to_string()),
    }
}

/// Add the submission in `body` to the registry, answering `{"ok": ..., "message": ...}`.
///
/// A body that is not a submission answers 400, a submission the registry cannot hold 409, and a registry that could
/// not be read or written 500. Each submission is logged with what became of it, the access log showing only the URL.
///
/// A body not sent as `application/json` answers 415 before it is read. A browser sends that type from another origin's
/// page only after a CORS preflight, which this server never grants, so a page elsewhere cannot write to the registry
/// by posting it a form or `text/plain`, the kinds of request a browser sends with no preflight. A body longer than
/// [`MAX_BODY`] answers 413 unread. Both are answered by [`submission_refused`], before this handler is called.
#[expect(
    clippy::future_not_send,
    reason = "actix runs each handler on its worker's own thread, and the request it takes is not Send"
)]
async fn register(
    request: HttpRequest,
    submission: web::Json<Submission>,
    registry: web::Data<RegistryTree>,
) -> HttpResponse {
    let submission = submission.into_inner();
    let peer = peer_of(&request);
    let outcome = registry.register(&submission);
    let message = outcome.as_ref().map_or_else(ToString::to_string, ToString::to_string);
    info!(
        "{peer} submitted {} {:?}: {message}",
        submission.interface, submission.label,
    );
    match outcome {
        Ok(_) => reply(HttpResponse::Ok(), true, &message),
        Err(error) if error.is_rejection() => reply(HttpResponse::Conflict(), false, &message),
        Err(_) => reply(HttpResponse::InternalServerError(), false, &message),
    }
}

/// Log the status lines a probe page posts in `body` once its run is over, answering 204.
///
/// A driver that cannot read the browser itself reads these from the server's log instead. The body is `{"status":
/// [line, ...]}`; one that is not answers 400, one not sent as `application/json` answers 415 unread, for the reason
/// [`register`] gives, and one longer than [`MAX_BODY`] 413 unread.
#[expect(
    clippy::future_not_send,
    reason = "actix runs each handler on its worker's own thread, and the request it takes is not Send"
)]
async fn status(request: HttpRequest, report: web::Json<StatusReport>) -> HttpResponse {
    let report = report.into_inner();
    let peer = peer_of(&request);
    for line in report.status {
        info!("{peer} page status: {line}");
    }
    HttpResponse::NoContent().finish()
}

/// The status lines a probe page posts once its run is over.
#[derive(serde::Deserialize)]
struct StatusReport {
    /// Each line of the page's status, in order.
    status: Vec<String>,
}

/// The address `request` came from, as the log names it.
fn peer_of(request: &HttpRequest) -> String {
    request
        .peer_addr()
        .map_or_else(|| "unknown peer".to_owned(), |address| address.ip().to_string())
}

/// The JSON answer to a submission.
fn reply(mut status: actix_web::HttpResponseBuilder, accepted: bool, message: &str) -> HttpResponse {
    status.json(json!({ "ok": accepted, "message": message }))
}

/// How a route reads its JSON body, with `refused` answering one it will not read or cannot read.
///
/// The body must be sent as `application/json` and be at most [`MAX_BODY`] long, and be what the route's handler takes.
/// actix checks the type and the length before it reads the body, and parses it only then.
fn json_body<Refused>(refused: Refused) -> web::JsonConfig
where
    Refused: Fn(JsonPayloadError, &HttpRequest) -> actix_web::Error + Send + Sync + 'static,
{
    web::JsonConfig::default()
        .limit(MAX_BODY)
        .content_type_required(true)
        .error_handler(refused)
}

/// The status a body that `error` kept from being read answers with.
///
/// That is 415 for one not sent as JSON, 413 for one past [`MAX_BODY`], and 400 for one that is not what the route
/// takes.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "JsonPayloadError is non_exhaustive; every error but the type and the length is a body it cannot read"
)]
fn refusal_status(error: &JsonPayloadError) -> actix_web::HttpResponseBuilder {
    match error {
        JsonPayloadError::ContentType => HttpResponse::UnsupportedMediaType(),
        JsonPayloadError::Overflow { .. } | JsonPayloadError::OverflowKnownLength { .. } => {
            HttpResponse::PayloadTooLarge()
        }
        _ => HttpResponse::BadRequest(),
    }
}

/// How `/register` answers a body it will not read or cannot read as a submission, in the JSON of every answer to one.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "JsonPayloadError is non_exhaustive; every error but the type and the length is a body it cannot read"
)]
fn submission_refused(error: JsonPayloadError, _request: &HttpRequest) -> actix_web::Error {
    let message = match &error {
        JsonPayloadError::ContentType => "a submission must be sent as application/json".to_owned(),
        JsonPayloadError::Overflow { .. } | JsonPayloadError::OverflowKnownLength { .. } => {
            "a submission must be at most 64 KiB".to_owned()
        }
        _ => format!("bad submission: {error}"),
    };
    let response = reply(refusal_status(&error), false, &message);
    InternalError::from_response(error, response).into()
}

/// How `/status` answers a body it will not read or cannot read as status lines, with no body.
fn status_refused(error: JsonPayloadError, _request: &HttpRequest) -> actix_web::Error {
    let response = refusal_status(&error).finish();
    InternalError::from_response(error, response).into()
}
