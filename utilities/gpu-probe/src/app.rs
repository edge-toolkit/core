//! The routes the server answers: the submission form, the endpoint it posts to, the registry, and any static files.
#![expect(
    clippy::single_call_fn,
    reason = "each route's handler is a named function, registered once by configure"
)]

use std::path::PathBuf;

use actix_web::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use actix_web::middleware::DefaultHeaders;
use actix_web::{HttpMessage as _, HttpRequest, HttpResponse, web};
use serde_json::json;
use tracing::info;

use crate::registry::{Registry, Submission};

/// The paste-and-register page served at `/submit`.
const SUBMIT_PAGE: &str = include_str!("submit.html");

/// The styles the server's pages share, served at `/probe.css`.
const STYLES: &str = include_str!("probe.css");

/// The module the server's pages post a submission with, served at `/register.js`.
const REGISTER_SCRIPT: &str = include_str!("register.mjs");

/// Add the server's routes to `config`, adding to `registry` and serving the files under `root`, if given, at `/`.
///
/// Nothing is cached: the registry changes while signatures are being collected, and a probe page being worked on
/// changes between visits.
pub fn configure(config: &mut web::ServiceConfig, registry: web::Data<Registry>, root: Option<PathBuf>) {
    let mut scope = web::scope("")
        .wrap(DefaultHeaders::new().add((CACHE_CONTROL, "no-store")))
        .app_data(registry)
        .route("/submit", embedded("text/html; charset=utf-8", SUBMIT_PAGE))
        .route("/probe.css", embedded("text/css; charset=utf-8", STYLES))
        .route(
            "/register.js",
            embedded("text/javascript; charset=utf-8", REGISTER_SCRIPT),
        )
        .route("/register", web::post().to(register))
        .route("/status", web::post().to(status))
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

/// The registry as it stands, for a probe page to say whether a device is in it already.
async fn signatures(registry: web::Data<Registry>) -> HttpResponse {
    match fs_err::read_to_string(registry.path()) {
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
/// A body not sent as `application/json` answers 415 before it is read. A browser sends that type from another
/// origin's page only after a CORS preflight, which this server never grants, so a page elsewhere cannot write to the
/// registry by posting it a form or `text/plain`, the kinds of request a browser sends with no preflight.
#[expect(
    clippy::future_not_send,
    reason = "actix runs each handler on its worker's own thread, and the request it takes is not Send"
)]
async fn register(request: HttpRequest, body: web::Bytes, registry: web::Data<Registry>) -> HttpResponse {
    if request.content_type() != "application/json" {
        let message = "a submission must be sent as application/json";
        return reply(HttpResponse::UnsupportedMediaType(), false, message);
    }
    let submission: Submission = match serde_json::from_slice(&body) {
        Ok(submission) => submission,
        Err(error) => return reply(HttpResponse::BadRequest(), false, &format!("bad submission: {error}")),
    };
    let peer = peer_of(&request);
    let outcome = registry.register(&submission);
    let message = outcome.as_ref().map_or_else(ToString::to_string, ToString::to_string);
    info!(
        "{peer} submitted {} {:?}: {message}",
        submission.interface, submission.label
    );
    match outcome {
        Ok(_) => reply(HttpResponse::Ok(), true, &message),
        Err(error) if error.is_rejection() => reply(HttpResponse::Conflict(), false, &message),
        Err(_) => reply(HttpResponse::InternalServerError(), false, &message),
    }
}

/// Log the status lines a probe page posts in `body` once its run is over, answering 204.
///
/// A driver that cannot read the browser itself reads these from the server's log instead. The body is
/// `{"status": [line, ...]}`; one that is not answers 400, and one not sent as `application/json` answers 415 unread,
/// for the reason [`register`] gives.
#[expect(
    clippy::future_not_send,
    reason = "actix runs each handler on its worker's own thread, and the request it takes is not Send"
)]
async fn status(request: HttpRequest, body: web::Bytes) -> HttpResponse {
    if request.content_type() != "application/json" {
        return HttpResponse::UnsupportedMediaType().finish();
    }
    let Ok(report) = serde_json::from_slice::<StatusReport>(&body) else {
        return HttpResponse::BadRequest().finish();
    };
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
