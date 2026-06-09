#![allow(rustdoc::invalid_rust_codeblocks)]
#[allow(unused_imports)]
pub use progenitor_client::{ByteStream, ClientInfo, Error, ResponseValue};
#[allow(unused_imports)]
use progenitor_client::{ClientHooks, OperationInfo, RequestBuilderExt, encode_path};
/// Types used as operation parameters and responses.
#[allow(clippy::all)]
pub mod types {
    /// Error types.
    pub mod error {
        /// Error from a `TryFrom` or `FromStr` implementation.
        pub struct ConversionError(::std::borrow::Cow<'static, str>);
        impl ::std::error::Error for ConversionError {}
        impl ::std::fmt::Display for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl ::std::fmt::Debug for ConversionError {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> Result<(), ::std::fmt::Error> {
                ::std::fmt::Debug::fmt(&self.0, f)
            }
        }
        impl From<&'static str> for ConversionError {
            fn from(value: &'static str) -> Self {
                Self(value.into())
            }
        }
        impl From<String> for ConversionError {
            fn from(value: String) -> Self {
                Self(value.into())
            }
        }
    }
    /**Server liveness probe response.

    Returned by `GET /health`.*/
    ///
    /// <details><summary>JSON schema</summary>
    ///
    /// ```json
    ///{
    ///  "description": "Server liveness probe response.\n\nReturned by `GET /health`.",
    ///  "type": "object",
    ///  "required": [
    ///    "service",
    ///    "status"
    ///  ],
    ///  "properties": {
    ///    "service": {
    ///      "type": "string"
    ///    },
    ///    "status": {
    ///      "type": "string"
    ///    }
    ///  }
    ///}
    /// ```
    /// </details>
    #[derive(::serde::Deserialize, ::serde::Serialize, Clone, Debug)]
    pub struct HealthResponse {
        pub service: ::std::string::String,
        pub status: ::std::string::String,
    }
}
#[derive(Clone, Debug)]
/**Client for Edge Toolkit REST API

ws-server HTTP surface: health probe, module discovery, module assets, per-agent storage.

Version: 0.1.0*/
pub struct Client {
    pub(crate) baseurl: String,
    pub(crate) client: reqwest::Client,
}
impl Client {
    /// Create a new client.
    ///
    /// `baseurl` is the base URL provided to the internal
    /// `reqwest::Client`, and should include a scheme and hostname,
    /// as well as port and a path stem if applicable.
    pub fn new(baseurl: &str) -> Self {
        #[cfg(target_arch = "wasm32")]
        let baseurl_owned = if baseurl.is_empty() {
            ::web_sys::window()
                .and_then(|w| w.location().origin().ok())
                .unwrap_or_default()
        } else {
            baseurl.to_string()
        };
        #[cfg(target_arch = "wasm32")]
        let baseurl = baseurl_owned.as_str();
        #[cfg(not(target_arch = "wasm32"))]
        let client = {
            let dur = ::std::time::Duration::from_secs(15u64);
            reqwest::ClientBuilder::new().connect_timeout(dur).timeout(dur)
        };
        #[cfg(target_arch = "wasm32")]
        let client = reqwest::ClientBuilder::new();
        Self::new_with_client(baseurl, client.build().unwrap())
    }
    /// Construct a new client with an existing `reqwest::Client`,
    /// allowing more control over its configuration.
    ///
    /// `baseurl` is the base URL provided to the internal
    /// `reqwest::Client`, and should include a scheme and hostname,
    /// as well as port and a path stem if applicable.
    pub fn new_with_client(baseurl: &str, client: reqwest::Client) -> Self {
        Self {
            baseurl: baseurl.to_string(),
            client,
        }
    }
}
impl ClientInfo<()> for Client {
    fn api_version() -> &'static str {
        "0.1.0"
    }
    fn baseurl(&self) -> &str {
        self.baseurl.as_str()
    }
    fn client(&self) -> &reqwest::Client {
        &self.client
    }
    fn inner(&self) -> &() {
        &()
    }
}
impl ClientHooks<()> for &Client {}
#[allow(clippy::all)]
impl Client {
    /**Liveness probe

    Returns a small JSON document identifying the service so external
    monitors can confirm the server is reachable and serving requests.

    Sends a `GET` request to `/health`

    */
    pub async fn health<'a>(&'a self) -> Result<ResponseValue<types::HealthResponse>, Error<()>> {
        let url = format!("{}/health", self.baseurl,);
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .headers(header_map)
            .build()?;
        let info = OperationInfo { operation_id: "health" };
        match (|request: &mut ::reqwest::Request| {
            #[cfg(feature = "tracing")]
            {
                let cx = <::tracing::Span as ::tracing_opentelemetry::OpenTelemetrySpanExt>::context(
                    &::tracing::Span::current(),
                );
                ::opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.inject_context(&cx, &mut ::opentelemetry_http::HeaderInjector(request.headers_mut()));
                });
            }
            #[cfg(not(feature = "tracing"))]
            let _ = request;
            async { Ok::<(), ::std::convert::Infallible>(()) }
        })(&mut request)
        .await
        {
            Ok(_) => {}
            Err(e) => return Err(Error::Custom(e.to_string())),
        }
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }
    /**List the names of every module the server is currently serving

    Sends a `GET` request to `/modules/`

    */
    pub async fn list_modules_handler<'a>(
        &'a self,
    ) -> Result<ResponseValue<::std::vec::Vec<::std::string::String>>, Error<()>> {
        let url = format!("{}/modules/", self.baseurl,);
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .get(url)
            .header(
                ::reqwest::header::ACCEPT,
                ::reqwest::header::HeaderValue::from_static("application/json"),
            )
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "list_modules_handler",
        };
        match (|request: &mut ::reqwest::Request| {
            #[cfg(feature = "tracing")]
            {
                let cx = <::tracing::Span as ::tracing_opentelemetry::OpenTelemetrySpanExt>::context(
                    &::tracing::Span::current(),
                );
                ::opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.inject_context(&cx, &mut ::opentelemetry_http::HeaderInjector(request.headers_mut()));
                });
            }
            #[cfg(not(feature = "tracing"))]
            let _ = request;
            async { Ok::<(), ::std::convert::Infallible>(()) }
        })(&mut request)
        .await
        {
            Ok(_) => {}
            Err(e) => return Err(Error::Custom(e.to_string())),
        }
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => ResponseValue::from_response(response).await,
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }
    /**Fetch a file from a module's bundled static assets

    `path` is resolved relative to the module's bundle root; an unknown
    module or missing file returns 404.

    Sends a `GET` request to `/modules/{name}/{path}`

    Arguments:
    - `name`: Module name
    - `path`: Path of the file within the module bundle
    */
    pub async fn get_module_file<'a>(
        &'a self,
        name: &'a str,
        path: &'a str,
    ) -> Result<ResponseValue<ByteStream>, Error<()>> {
        let url = format!(
            "{}/modules/{}/{}",
            self.baseurl,
            encode_path(&name.to_string()),
            encode_path(&path.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self.client.get(url).headers(header_map).build()?;
        let info = OperationInfo {
            operation_id: "get_module_file",
        };
        match (|request: &mut ::reqwest::Request| {
            #[cfg(feature = "tracing")]
            {
                let cx = <::tracing::Span as ::tracing_opentelemetry::OpenTelemetrySpanExt>::context(
                    &::tracing::Span::current(),
                );
                ::opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.inject_context(&cx, &mut ::opentelemetry_http::HeaderInjector(request.headers_mut()));
                });
            }
            #[cfg(not(feature = "tracing"))]
            let _ = request;
            async { Ok::<(), ::std::convert::Infallible>(()) }
        })(&mut request)
        .await
        {
            Ok(_) => {}
            Err(e) => return Err(Error::Custom(e.to_string())),
        }
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => Ok(ResponseValue::stream(response)),
            404u16 => Err(Error::ErrorResponse(ResponseValue::empty(response))),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }
    /**Download a file previously written to the named agent's storage bucket

    Sends a `GET` request to `/storage/{agent_id}/{filename}`

    Arguments:
    - `agent_id`: Agent identifier
    - `filename`: Stored filename
    */
    pub async fn get_file<'a>(
        &'a self,
        agent_id: &'a str,
        filename: &'a str,
    ) -> Result<ResponseValue<ByteStream>, Error<()>> {
        let url = format!(
            "{}/storage/{}/{}",
            self.baseurl,
            encode_path(&agent_id.to_string()),
            encode_path(&filename.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self.client.get(url).headers(header_map).build()?;
        let info = OperationInfo {
            operation_id: "get_file",
        };
        match (|request: &mut ::reqwest::Request| {
            #[cfg(feature = "tracing")]
            {
                let cx = <::tracing::Span as ::tracing_opentelemetry::OpenTelemetrySpanExt>::context(
                    &::tracing::Span::current(),
                );
                ::opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.inject_context(&cx, &mut ::opentelemetry_http::HeaderInjector(request.headers_mut()));
                });
            }
            #[cfg(not(feature = "tracing"))]
            let _ = request;
            async { Ok::<(), ::std::convert::Infallible>(()) }
        })(&mut request)
        .await
        {
            Ok(_) => {}
            Err(e) => return Err(Error::Custom(e.to_string())),
        }
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => Ok(ResponseValue::stream(response)),
            404u16 => Err(Error::ErrorResponse(ResponseValue::empty(response))),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }
    /**Upload a file to an agent's storage bucket

    Only the agent that owns the bucket may write to it (the agent must
    currently be connected); the path component must be a single
    filename, not a nested path.

    Sends a `PUT` request to `/storage/{agent_id}/{filename}`

    Arguments:
    - `agent_id`: Agent identifier (must be a connected agent)
    - `filename`: Single-segment filename to write
    - `body`: Raw file bytes
    */
    pub async fn put_file<'a, B: Into<reqwest::Body>>(
        &'a self,
        agent_id: &'a str,
        filename: &'a str,
        body: B,
    ) -> Result<ResponseValue<()>, Error<()>> {
        let url = format!(
            "{}/storage/{}/{}",
            self.baseurl,
            encode_path(&agent_id.to_string()),
            encode_path(&filename.to_string()),
        );
        let mut header_map = ::reqwest::header::HeaderMap::with_capacity(1usize);
        header_map.append(
            ::reqwest::header::HeaderName::from_static("api-version"),
            ::reqwest::header::HeaderValue::from_static(Self::api_version()),
        );
        #[allow(unused_mut)]
        let mut request = self
            .client
            .put(url)
            .header(
                ::reqwest::header::CONTENT_TYPE,
                ::reqwest::header::HeaderValue::from_static("application/octet-stream"),
            )
            .body(body)
            .headers(header_map)
            .build()?;
        let info = OperationInfo {
            operation_id: "put_file",
        };
        match (|request: &mut ::reqwest::Request| {
            #[cfg(feature = "tracing")]
            {
                let cx = <::tracing::Span as ::tracing_opentelemetry::OpenTelemetrySpanExt>::context(
                    &::tracing::Span::current(),
                );
                ::opentelemetry::global::get_text_map_propagator(|propagator| {
                    propagator.inject_context(&cx, &mut ::opentelemetry_http::HeaderInjector(request.headers_mut()));
                });
            }
            #[cfg(not(feature = "tracing"))]
            let _ = request;
            async { Ok::<(), ::std::convert::Infallible>(()) }
        })(&mut request)
        .await
        {
            Ok(_) => {}
            Err(e) => return Err(Error::Custom(e.to_string())),
        }
        self.pre(&mut request, &info).await?;
        let result = self.exec(request, &info).await;
        self.post(&result, &info).await?;
        let response = result?;
        match response.status().as_u16() {
            200u16 => Ok(ResponseValue::empty(response)),
            400u16 => Err(Error::ErrorResponse(ResponseValue::empty(response))),
            404u16 => Err(Error::ErrorResponse(ResponseValue::empty(response))),
            _ => Err(Error::UnexpectedResponse(response)),
        }
    }
}
/// Items consumers will typically use such as the Client.
pub mod prelude {
    #[allow(unused_imports)]
    pub use super::Client;
}
