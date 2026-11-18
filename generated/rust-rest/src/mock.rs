pub mod operations {
    //! [`When`](::httpmock::When) and [`Then`](::httpmock::Then)
    //! wrappers for each operation. Each can be converted to
    //! its inner type with a call to `into_inner()`. This can
    //! be used to explicitly deviate from permitted values.
    use crate::*;
    pub struct EvictAgentWhen(::httpmock::When);
    impl EvictAgentWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(
                inner
                    .method(::httpmock::Method::DELETE)
                    .path_matches(regex::Regex::new("^/agents/[^/]*$").unwrap()),
            )
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
        pub fn agent_id(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/agents/{}$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
    }
    pub struct EvictAgentThen(::httpmock::Then);
    impl EvictAgentThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self, value: &types::EvictedAgent) -> Self {
            Self(
                self.0
                    .status(200u16)
                    .header("content-type", "application/json")
                    .json_body_obj(value),
            )
        }
        pub fn unauthorized(self) -> Self {
            Self(self.0.status(401u16))
        }
        pub fn forbidden(self) -> Self {
            Self(self.0.status(403u16))
        }
        pub fn not_found(self) -> Self {
            Self(self.0.status(404u16))
        }
        pub fn service_unavailable(self) -> Self {
            Self(self.0.status(503u16))
        }
    }
    pub struct BlockIpWhen(::httpmock::When);
    impl BlockIpWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(
                inner
                    .method(::httpmock::Method::PUT)
                    .path_matches(regex::Regex::new("^/blocked-ips/[^/]*$").unwrap()),
            )
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
        pub fn ip(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/blocked-ips/{}$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
    }
    pub struct BlockIpThen(::httpmock::Then);
    impl BlockIpThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn no_content(self) -> Self {
            Self(self.0.status(204u16))
        }
        pub fn unauthorized(self) -> Self {
            Self(self.0.status(401u16))
        }
        pub fn forbidden(self) -> Self {
            Self(self.0.status(403u16))
        }
        pub fn not_found(self) -> Self {
            Self(self.0.status(404u16))
        }
        pub fn service_unavailable(self) -> Self {
            Self(self.0.status(503u16))
        }
    }
    pub struct HealthWhen(::httpmock::When);
    impl HealthWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(inner.method(::httpmock::Method::GET).path("/health"))
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
    }
    pub struct HealthThen(::httpmock::Then);
    impl HealthThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self, value: &types::HealthResponse) -> Self {
            Self(
                self.0
                    .status(200u16)
                    .header("content-type", "application/json")
                    .json_body_obj(value),
            )
        }
    }
    pub struct InfoWhen(::httpmock::When);
    impl InfoWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(inner.method(::httpmock::Method::GET).path("/info"))
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
    }
    pub struct InfoThen(::httpmock::Then);
    impl InfoThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self, value: &types::BuildInfo) -> Self {
            Self(
                self.0
                    .status(200u16)
                    .header("content-type", "application/json")
                    .json_body_obj(value),
            )
        }
    }
    pub struct ListModulesHandlerWhen(::httpmock::When);
    impl ListModulesHandlerWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(inner.method(::httpmock::Method::GET).path("/modules/"))
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
    }
    pub struct ListModulesHandlerThen(::httpmock::Then);
    impl ListModulesHandlerThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self, value: &::std::vec::Vec<::std::string::String>) -> Self {
            Self(
                self.0
                    .status(200u16)
                    .header("content-type", "application/json")
                    .json_body_obj(value),
            )
        }
    }
    pub struct GetModuleFileWhen(::httpmock::When);
    impl GetModuleFileWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(
                inner
                    .method(::httpmock::Method::GET)
                    .path_matches(regex::Regex::new("^/modules/[^/]*/[^/]*$").unwrap()),
            )
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
        pub fn name(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/modules/{}/.*$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
        pub fn path(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/modules/.*/{}$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
    }
    pub struct GetModuleFileThen(::httpmock::Then);
    impl GetModuleFileThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self, value: ::serde_json::Value) -> Self {
            Self(
                self.0
                    .status(200u16)
                    .header("content-type", "application/json")
                    .json_body(value),
            )
        }
        pub fn not_found(self) -> Self {
            Self(self.0.status(404u16))
        }
    }
    pub struct GetFileWhen(::httpmock::When);
    impl GetFileWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(
                inner
                    .method(::httpmock::Method::GET)
                    .path_matches(regex::Regex::new("^/storage/[^/]*/[^/]*$").unwrap()),
            )
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
        pub fn agent_id(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/storage/{}/.*$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
        pub fn filename(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/storage/.*/{}$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
    }
    pub struct GetFileThen(::httpmock::Then);
    impl GetFileThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self, value: ::serde_json::Value) -> Self {
            Self(
                self.0
                    .status(200u16)
                    .header("content-type", "application/json")
                    .json_body(value),
            )
        }
        pub fn forbidden(self) -> Self {
            Self(self.0.status(403u16))
        }
        pub fn not_found(self) -> Self {
            Self(self.0.status(404u16))
        }
    }
    pub struct PutFileWhen(::httpmock::When);
    impl PutFileWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(
                inner
                    .method(::httpmock::Method::PUT)
                    .path_matches(regex::Regex::new("^/storage/[^/]*/[^/]*$").unwrap()),
            )
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
        pub fn agent_id(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/storage/{}/.*$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
        pub fn filename(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/storage/.*/{}$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
        pub fn body(self, value: ::serde_json::Value) -> Self {
            Self(self.0.json_body(value))
        }
    }
    pub struct PutFileThen(::httpmock::Then);
    impl PutFileThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self) -> Self {
            Self(self.0.status(200u16))
        }
        pub fn bad_request(self) -> Self {
            Self(self.0.status(400u16))
        }
        pub fn forbidden(self) -> Self {
            Self(self.0.status(403u16))
        }
        pub fn not_found(self) -> Self {
            Self(self.0.status(404u16))
        }
    }
    pub struct HeadFileWhen(::httpmock::When);
    impl HeadFileWhen {
        pub fn new(inner: ::httpmock::When) -> Self {
            Self(
                inner
                    .method(::httpmock::Method::HEAD)
                    .path_matches(regex::Regex::new("^/storage/[^/]*/[^/]*$").unwrap()),
            )
        }
        pub fn into_inner(self) -> ::httpmock::When {
            self.0
        }
        pub fn agent_id(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/storage/{}/.*$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
        pub fn filename(self, value: &str) -> Self {
            let re = regex::Regex::new(&format!("^/storage/.*/{}$", regex::escape(value))).unwrap();
            Self(self.0.path_matches(re))
        }
    }
    pub struct HeadFileThen(::httpmock::Then);
    impl HeadFileThen {
        pub fn new(inner: ::httpmock::Then) -> Self {
            Self(inner)
        }
        pub fn into_inner(self) -> ::httpmock::Then {
            self.0
        }
        pub fn ok(self) -> Self {
            Self(self.0.status(200u16))
        }
        pub fn forbidden(self) -> Self {
            Self(self.0.status(403u16))
        }
        pub fn not_found(self) -> Self {
            Self(self.0.status(404u16))
        }
    }
}
/// An extension trait for [`MockServer`](::httpmock::MockServer) that
/// adds a method for each operation. These are the equivalent of
/// type-checked [`mock()`](::httpmock::MockServer::mock) calls.
pub trait MockServerExt {
    fn evict_agent<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::EvictAgentWhen, operations::EvictAgentThen);
    fn block_ip<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::BlockIpWhen, operations::BlockIpThen);
    fn health<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::HealthWhen, operations::HealthThen);
    fn info<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::InfoWhen, operations::InfoThen);
    fn list_modules_handler<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::ListModulesHandlerWhen, operations::ListModulesHandlerThen);
    fn get_module_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::GetModuleFileWhen, operations::GetModuleFileThen);
    fn get_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::GetFileWhen, operations::GetFileThen);
    fn put_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::PutFileWhen, operations::PutFileThen);
    fn head_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::HeadFileWhen, operations::HeadFileThen);
}
impl MockServerExt for ::httpmock::MockServer {
    fn evict_agent<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::EvictAgentWhen, operations::EvictAgentThen),
    {
        self.mock(|when, then| {
            config_fn(
                operations::EvictAgentWhen::new(when),
                operations::EvictAgentThen::new(then),
            )
        })
    }
    fn block_ip<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::BlockIpWhen, operations::BlockIpThen),
    {
        self.mock(|when, then| config_fn(operations::BlockIpWhen::new(when), operations::BlockIpThen::new(then)))
    }
    fn health<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::HealthWhen, operations::HealthThen),
    {
        self.mock(|when, then| config_fn(operations::HealthWhen::new(when), operations::HealthThen::new(then)))
    }
    fn info<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::InfoWhen, operations::InfoThen),
    {
        self.mock(|when, then| config_fn(operations::InfoWhen::new(when), operations::InfoThen::new(then)))
    }
    fn list_modules_handler<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::ListModulesHandlerWhen, operations::ListModulesHandlerThen),
    {
        self.mock(|when, then| {
            config_fn(
                operations::ListModulesHandlerWhen::new(when),
                operations::ListModulesHandlerThen::new(then),
            )
        })
    }
    fn get_module_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::GetModuleFileWhen, operations::GetModuleFileThen),
    {
        self.mock(|when, then| {
            config_fn(
                operations::GetModuleFileWhen::new(when),
                operations::GetModuleFileThen::new(then),
            )
        })
    }
    fn get_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::GetFileWhen, operations::GetFileThen),
    {
        self.mock(|when, then| config_fn(operations::GetFileWhen::new(when), operations::GetFileThen::new(then)))
    }
    fn put_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::PutFileWhen, operations::PutFileThen),
    {
        self.mock(|when, then| config_fn(operations::PutFileWhen::new(when), operations::PutFileThen::new(then)))
    }
    fn head_file<F>(&self, config_fn: F) -> ::httpmock::Mock<'_>
    where
        F: FnOnce(operations::HeadFileWhen, operations::HeadFileThen),
    {
        self.mock(|when, then| config_fn(operations::HeadFileWhen::new(when), operations::HeadFileThen::new(then)))
    }
}
