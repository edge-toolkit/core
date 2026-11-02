//! Modules built into the binary, served through picoserve over real TCP sockets, with tokio standing in for an
//! embedded executor and network stack: listed under `/modules/`, each file under `/modules/{name}/`, the root module's
//! from `/`, and a module served elsewhere redirected there.
#![cfg(test)]

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use et_modules_service::picoserve::{Module, ModuleFile, ModuleRoutes, Modules};
use picoserve::{Config, Router, Server};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::LocalSet;

/// How long a test waits for an answer before failing rather than hanging.
const PATIENCE: Duration = Duration::from_secs(5);

/// A page served at `/`, a scoped module beside it, and a package redirected to a CDN.
static MODULES: Modules = Modules::new(
    &[
        Module::new(
            "page",
            &[
                ("index.html", ModuleFile::new("text/html", b"<p>page</p>", "index")),
                ("app.js", ModuleFile::new("text/javascript", b"app", "app")),
            ],
        ),
        Module::new(
            "@scope/math",
            &[
                ("package.json", ModuleFile::new("application/json", b"{}", "package")),
                ("pkg/math.wasm", ModuleFile::new("application/wasm", b"\0asm", "wasm")),
            ],
        ),
    ],
    "page",
    &["big"],
    "https://cdn.example/npm",
);

/// `modules` served on a fresh loopback port, one task per connection as an embedder would, and its address.
async fn served(modules: &'static Modules) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let _accepting = tokio::task::spawn_local(async move {
        let app = Router::from_service(ModuleRoutes::new(modules));
        let config = Config::const_default();
        loop {
            let (socket, _peer) = listener.accept().await.unwrap();
            let mut http_buffer = [0; 2048];
            let _served = Server::new_tokio(&app, &config, &mut http_buffer).serve(socket).await;
        }
    });
    address
}

/// The whole response to `head`, a request line and headers to which the closing ones are added.
async fn send(address: SocketAddr, head: &str) -> String {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let request = format!("{head}Connection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::default();
    let _read = tokio::time::timeout(PATIENCE, socket.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    response
}

/// The whole response to a bodiless `method path`.
async fn request(address: SocketAddr, method: &str, path: &str) -> String {
    send(address, &format!("{method} {path} HTTP/1.1\r\nHost: hub\r\n")).await
}

/// The whole response to a `GET path`.
async fn get(address: SocketAddr, path: &str) -> String {
    request(address, "GET", path).await
}

/// The body of `response`, after its head.
fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").unwrap().1
}

#[tokio::test]
async fn it_serves_the_embedded_modules() {
    LocalSet::new()
        .run_until(Box::pin(async {
            let address = served(&MODULES).await;

            let list = get(address, "/modules/").await;
            assert!(list.starts_with("HTTP/1.1 200"));
            let names: Vec<String> = serde_json::from_str(body(&list)).unwrap();
            assert_eq!(names, ["@scope/math", "big", "page"]);
            assert!(get(address, "/modules").await.starts_with("HTTP/1.1 200"));

            let page = get(address, "/").await;
            assert!(page.contains("Content-Type: text/html"));
            assert_eq!(body(&page), "<p>page</p>");
            assert_eq!(body(&get(address, "/app.js").await), "app");
            assert_eq!(body(&get(address, "/modules/page/app.js").await), "app");
            assert!(get(address, "/modules/page/").await.starts_with("HTTP/1.1 404"));

            let wasm = get(address, "/modules/@scope/math/pkg/math.wasm").await;
            assert!(wasm.contains("Content-Type: application/wasm"));
            assert!(wasm.contains("ETag: \"wasm\"\r\n"));
            assert_eq!(body(&wasm), "\0asm");
            let wasm_path = "GET /modules/@scope/math/pkg/math.wasm HTTP/1.1\r\nHost: hub\r\n";
            for tags in ["\"wasm\"", "\"other\", W/\"wasm\"", "*"] {
                let revalidated = send(address, &format!("{wasm_path}If-None-Match: {tags}\r\n")).await;
                assert!(revalidated.starts_with("HTTP/1.1 304"));
            }
            let changed = send(address, &format!("{wasm_path}If-None-Match: \"other\", wasm\r\n")).await;
            assert_eq!(body(&changed), "\0asm");
            let encoded = get(address, "/modules/%40scope/math/package.json").await;
            assert_eq!(body(&encoded), "{}");
            for missing in [
                "/modules/@scope/math/missing.js",
                "/modules/@scope/mathematics/x",
                "/modulesx",
                "/modules/bigger/x.js",
            ] {
                assert!(get(address, missing).await.starts_with("HTTP/1.1 404"));
            }

            let redirect = get(address, "/modules/big/dist/big.js").await;
            assert!(redirect.starts_with("HTTP/1.1 307"));
            assert!(redirect.contains("Location: https://cdn.example/npm/big/dist/big.js\r\n"));

            assert!(request(address, "POST", "/app.js").await.starts_with("HTTP/1.1 405"));
            assert!(request(address, "POST", "/missing").await.starts_with("HTTP/1.1 404"));
        }))
        .await;
}

#[tokio::test]
async fn modules_assembled_at_run_time_serve_as_built_in_ones_do() {
    // Read from somewhere only known once running, rather than built in, and so leaked to live as long as the server.
    let file = ModuleFile::new("text/plain", Box::leak(b"run".to_vec().into_boxed_slice()), "run");
    let files: &'static [(&'static str, ModuleFile)] = Box::leak(Box::new([("a.txt", file)]));
    let module = Module::new("assembled", files);
    // No page at `/`, and a module named as served elsewhere with nowhere to send it.
    let modules: &'static Modules = Box::leak(Box::new(Modules::new(Box::leak(Box::new([module])), "", &["far"], "")));
    LocalSet::new()
        .run_until(Box::pin(async {
            let address = served(modules).await;
            assert_eq!(body(&get(address, "/modules/assembled/a.txt").await), "run");
            assert!(get(address, "/").await.starts_with("HTTP/1.1 404"));
            assert!(get(address, "/modules/far/x.js").await.starts_with("HTTP/1.1 404"));
            let names: Vec<String> = serde_json::from_str(body(&get(address, "/modules/").await)).unwrap();
            assert_eq!(names, ["assembled", "far"]);
        }))
        .await;
}

#[test]
fn no_modules_serve_nothing() {
    assert!(Modules::NONE.embedded.is_empty() && Modules::NONE.root.is_empty());
}
