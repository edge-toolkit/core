use anyhow::Context;
use et_ws_worker::{apply_browser_polyfills, create_runtime, derive_http_base};
use rustyscript::{Module, Undefined, json_args};
use tracing::info;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let module_name = std::env::var("WORKER_MODULE").context("WORKER_MODULE not set")?;
    let ws_url = std::env::var("WS_SERVER_URL").unwrap_or_else(|_| {
        format!(
            "ws://localhost:{}/ws",
            edge_toolkit::ports::Services::InsecureWebSocketServer.port()
        )
    });
    let http_base = derive_http_base(&ws_url).context("could not derive HTTP base URL from WS_SERVER_URL")?;

    info!("et-ws-worker: module={module_name} server={http_base}");

    let mut runtime = create_runtime(&http_base)?;
    let tokio_runtime = runtime.tokio_runtime();

    let pkg_url = format!("{http_base}/modules/{module_name}/package.json");
    let pkg: serde_json::Value = tokio_runtime.block_on(async {
        reqwest::Client::new()
            .get(&pkg_url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    })?;
    let main_file = pkg["main"]
        .as_str()
        .with_context(|| format!("module {module_name} package.json missing 'main'"))?;

    let entry_url = format!("{http_base}/modules/{module_name}/{main_file}");
    info!("loading entry: {entry_url}");

    apply_browser_polyfills(&mut runtime, &http_base)?;

    let stub = Module::new("entry.js", format!(r#"export {{ default, run }} from {entry_url:?};"#));
    let handle = runtime.load_module(&stub)?;

    tokio_runtime.block_on(async {
        runtime
            .call_function_async::<Undefined>(Some(&handle), "default", json_args!())
            .await?;
        runtime
            .call_function_async::<Undefined>(Some(&handle), "run", json_args!())
            .await?;
        Ok::<(), rustyscript::Error>(())
    })?;

    info!("module {module_name} completed successfully");
    Ok(())
}
