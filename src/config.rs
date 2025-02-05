//! Agent configuration

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use http::Uri;
use ureq_proto::client::flow::RedirectAuthHeaders;

use crate::http;
use crate::middleware::{Middleware, MiddlewareChain};
use crate::resolver::IpFamily;
use crate::{Agent, AsSendBody, Proxy, RequestBuilder};

#[cfg(feature = "_tls")]
use crate::tls::TlsConfig;

/// Config primarily for the [`Agent`], but also per-request.
///
/// Config objects are cheap to clone and should not incur any heap allocations.
///
/// # Agent level config
///
/// ## Example
///
/// ```
/// use ureq::Agent;
/// use std::time::Duration;
///
/// let config = Agent::config_builder()
///     .timeout_global(Some(Duration::from_secs(10)))
///     .https_only(true)
///     .build();
///
/// let agent = Agent::new_with_config(config);
/// ```
///
///
/// # Request level config
///
/// The config can also be change per-request. Since every request ultimately executes
/// using an [`Agent`] (also the root-level `ureq::get(...)` have an implicit agent),
/// a request level config clones the agent level config.
///
/// There are two ways of getting a request level config.
///
/// ## Request builder example
///
/// The first way is via [`RequestBuilder::config()`][crate::RequestBuilder::config].
///
/// ```
/// use ureq::Agent;
///
/// let agent: Agent = Agent::config_builder()
///     .https_only(false)
///     .build()
///     .into();
///
/// let response = agent.get("http://httpbin.org/get")
///     .config()
///     // override agent level setting for this request
///     .https_only(true)
///     .build()
///     .call();
/// ```
///
/// ## HTTP request example
///
/// The second way is via [`Agent::configure_request()`][crate::Agent::configure_request].
/// This is used when working with the http crate [`http::Request`] type directly.
///
/// ```
/// use ureq::{http, Agent};
///
/// let agent: Agent = Agent::config_builder()
///     .https_only(false)
///     .build()
///     .into();
///
/// let request = http::Request::get("http://httpbin.org/get")
///     .body(()).unwrap();
///
/// let request = agent.configure_request(request)
///     // override agent level setting for this request
///     .https_only(true)
///     .build();
///
/// let response = agent.run(request);
/// ```
///
#[derive(Clone)]
pub struct Config {
    pub(crate) http_status_as_error: bool,
    pub(crate) https_only: bool,
    pub(crate) ip_family: IpFamily,
    #[cfg(feature = "_tls")]
    pub(crate) tls_config: TlsConfig,
    pub(crate) proxy: Option<Proxy>,
    pub(crate) no_delay: bool,
    pub(crate) max_redirects: u32,
    pub(crate) redirect_auth_headers: RedirectAuthHeaders,
    pub(crate) user_agent: AutoHeaderValue,
    pub(crate) accept: AutoHeaderValue,
    pub(crate) accept_encoding: AutoHeaderValue,
    pub(crate) timeouts: Timeouts,
    pub(crate) max_response_header_size: usize,
    pub(crate) input_buffer_size: usize,
    pub(crate) output_buffer_size: usize,
    pub(crate) max_idle_connections: usize,
    pub(crate) max_idle_connections_per_host: usize,
    pub(crate) max_idle_age: Duration,
    pub(crate) middleware: MiddlewareChain,

    // Techically not config, but here to pass as argument from
    // RequestBuilder::force_send_body() to run()
    pub(crate) force_send_body: bool,
}

impl Config {
    /// A builder to make a bespoke configuration.
    ///
    /// The default values are already set.
    pub fn builder() -> ConfigBuilder<AgentScope> {
        ConfigBuilder(AgentScope(Config::default()))
    }

    /// Creates a new agent by cloning this config.
    ///
    /// Cloning the config does not incur heap allocations.
    pub fn new_agent(&self) -> Agent {
        self.clone().into()
    }

    pub(crate) fn connect_proxy_uri(&self) -> Option<&Uri> {
        let proxy = self.proxy.as_ref()?;

        if !proxy.proto().is_connect() {
            return None;
        }

        Some(proxy.uri())
    }
}

/// Builder of [`Config`]
pub struct ConfigBuilder<Scope: private::ConfigScope>(pub(crate) Scope);

#[doc(hidden)]
pub struct AgentScope(Config);
#[doc(hidden)]
pub struct RequestScope<Any>(pub(crate) RequestBuilder<Any>);
#[doc(hidden)]
pub struct HttpCrateScope<S: AsSendBody>(pub(crate) http::Request<S>);

impl private::ConfigScope for AgentScope {
    fn config(&mut self) -> &mut Config {
        &mut self.0
    }
}

impl<Any> private::ConfigScope for RequestScope<Any> {
    fn config(&mut self) -> &mut Config {
        self.0.request_level_config()
    }
}

impl<S: AsSendBody> private::ConfigScope for HttpCrateScope<S> {
    fn config(&mut self) -> &mut Config {
        // This unwrap is OK, because we should not construct an
        // HttpCrateScope without first ensure it is there.
        let req_level: &mut RequestLevelConfig = self.0.extensions_mut().get_mut().unwrap();
        &mut req_level.0
    }
}

mod private {
    use super::Config;

    pub trait ConfigScope {
        fn config(&mut self) -> &mut Config;
    }
}

impl<Scope: private::ConfigScope> ConfigBuilder<Scope> {
    fn config(&mut self) -> &mut Config {
        self.0.config()
    }

    /// Whether to treat 4xx and 5xx HTTP status codes as
    /// [`Err(Error::StatusCode))`](crate::Error::StatusCode).
    ///
    /// Defaults to `true`.
    pub fn http_status_as_error(mut self, v: bool) -> Self {
        self.config().http_status_as_error = v;
        self
    }

    /// Whether to limit requests (including redirects) to https only
    ///
    /// Defaults to `false`.
    pub fn https_only(mut self, v: bool) -> Self {
        self.config().https_only = v;
        self
    }

    /// Configuration of IPv4/IPv6.
    ///
    /// This affects the resolver.
    ///
    /// Defaults to `IpFamily::Any`.
    pub fn ip_family(mut self, v: IpFamily) -> Self {
        self.config().ip_family = v;
        self
    }

    /// Config for TLS.
    ///
    /// This config is generic for all TLS connectors.
    #[cfg(feature = "_tls")]
    pub fn tls_config(mut self, v: TlsConfig) -> Self {
        self.config().tls_config = v;
        self
    }

    /// Proxy configuration.
    ///
    /// Picked up from environment when using [`Config::default()`] or
    /// [`Agent::new_with_defaults()`][crate::Agent::new_with_defaults].
    pub fn proxy(mut self, v: Option<Proxy>) -> Self {
        self.config().proxy = v;
        self
    }

    /// Disable Nagle's algorithm
    ///
    /// Set TCP_NODELAY. It's up to the transport whether this flag is honored.
    ///
    /// Defaults to `true`.
    pub fn no_delay(mut self, v: bool) -> Self {
        self.config().no_delay = v;
        self
    }

    /// The max number of redirects to follow before giving up
    ///
    /// Defaults to 10
    pub fn max_redirects(mut self, v: u32) -> Self {
        self.config().max_redirects = v;
        self
    }

    /// How to handle `Authorization` headers when following redirects
    ///
    /// * `Never` (the default) means the authorization header is never attached to a redirected call.
    /// * `SameHost` will keep the header when the redirect is to the same host and under https.
    ///
    /// Defaults to `None`.
    pub fn redirect_auth_headers(mut self, v: RedirectAuthHeaders) -> Self {
        self.config().redirect_auth_headers = v;
        self
    }

    /// Value to use for the `User-Agent` header.
    ///
    /// This can be overridden by setting a `user-agent` header on the request
    /// object. The one difference is that a connection to a HTTP proxy server
    /// will receive this value, not the request-level one.
    ///
    /// Setting a value of `""` on the request or agent level will also not send a header.
    ///
    /// Defaults to `Default`, which results in `ureq/<version>`
    pub fn user_agent(mut self, v: AutoHeaderValue) -> Self {
        self.config().user_agent = v;
        self
    }

    /// Value to use for the `Accept` header.
    ///
    /// This agent configured value can be overriden per request by setting the header.
    //
    /// Setting a value of `""` on the request or agent level will also not send a header.
    ///
    /// Defaults to `Default`, which results in `*/*`
    pub fn accept(mut self, v: AutoHeaderValue) -> Self {
        self.config().accept = v;
        self
    }

    /// Value to use for the `Accept-Encoding` header.
    ///
    /// Defaults to `Default`, which will add `gz` and `brotli` depending on
    /// the feature flags **gzip** and **brotli** respectively. If neither
    /// feature is enabled, the header is not added.
    ///
    /// This agent configured value can be overriden per request by setting the header.
    ///
    /// Setting a value of `""` on the request or agent level will also not send a header.
    ///
    /// This communicates capability to the server, however the triggering the
    /// automatic decompression behavior is not affected since that only looks
    /// at the `Content-Encoding` response header.
    pub fn accept_encoding(mut self, v: AutoHeaderValue) -> Self {
        self.config().accept_encoding = v;
        self
    }

    /// Max size of the HTTP response header.
    ///
    /// From the status, including all headers up until the body.
    ///
    /// Defaults to 64kb.
    pub fn max_response_header_size(mut self, v: usize) -> Self {
        self.config().max_response_header_size = v;
        self
    }

    /// Default size of the input buffer
    ///
    /// The default connectors use this setting.
    ///
    /// Defaults to 128kb.
    pub fn input_buffer_size(mut self, v: usize) -> Self {
        self.config().input_buffer_size = v;
        self
    }

    /// Default size of the output buffer.
    ///
    /// The default connectors use this setting.
    ///
    /// Defaults to 128kb.
    pub fn output_buffer_size(mut self, v: usize) -> Self {
        self.config().output_buffer_size = v;
        self
    }

    /// Max number of idle pooled connections overall.
    ///
    /// This setting has no effect when used per-request.
    ///
    /// Defaults to 10
    pub fn max_idle_connections(mut self, v: usize) -> Self {
        self.config().max_idle_connections = v;
        self
    }

    /// Max number of idle pooled connections per host/port combo.
    ///
    /// This setting has no effect when used per-request.
    ///
    /// Defaults to 3
    pub fn max_idle_connections_per_host(mut self, v: usize) -> Self {
        self.config().max_idle_connections_per_host = v;
        self
    }

    /// Max duration to keep an idle connection in the pool
    ///
    /// This can also be configured per-request to be shorter than the pool.
    /// For example: if the pool is configured to 15 seconds and we have a
    /// connection with an age of 10 seconds, a request setting this config
    /// property to 3 seconds, would ignore the pooled connection (but still
    /// leave it in the pool).
    ///
    /// Defaults to 15 seconds
    pub fn max_idle_age(mut self, v: Duration) -> Self {
        self.config().max_idle_age = v;
        self
    }

    /// Add middleware to use for each request in this agent.
    ///
    /// Defaults to no middleware.
    pub fn middleware(mut self, v: impl Middleware) -> Self {
        self.config().middleware.add(v);
        self
    }

    /// Timeout for the entire call
    ///
    /// This is end-to-end, from DNS lookup to finishing reading the response body.
    /// Thus it covers all other timeouts.
    ///
    /// Defaults to `None`.
    pub fn timeout_global(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.global = v;
        self
    }

    /// Timeout for call-by-call when following redirects
    ///
    /// This covers a single call and the timeout is reset when
    /// ureq follows a redirections.
    ///
    /// Defaults to `None`..
    pub fn timeout_per_call(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.per_call = v;
        self
    }

    /// Max duration for doing the DNS lookup when establishing the connection
    ///
    /// Because most platforms do not have an async syscall for looking up
    /// a host name, setting this might force str0m to spawn a thread to handle
    /// the timeout.
    ///
    /// Defaults to `None`.
    pub fn timeout_resolve(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.resolve = v;
        self
    }

    /// Max duration for establishing the connection
    ///
    /// For a TLS connection this includes opening the socket and doing the TLS handshake.
    ///
    /// Defaults to `None`.
    pub fn timeout_connect(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.connect = v;
        self
    }

    /// Max duration for sending the request, but not the request body.
    ///
    /// Defaults to `None`.
    pub fn timeout_send_request(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.send_request = v;
        self
    }

    /// Max duration for awaiting a 100-continue response.
    ///
    /// Only used if there is a request body and we sent the `Expect: 100-continue`
    /// header to indicate we want the server to respond with 100.
    ///
    /// This defaults to 1 second.
    pub fn timeout_await_100(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.await_100 = v;
        self
    }

    /// Max duration for sending a request body (if there is one)
    ///
    /// Defaults to `None`.
    pub fn timeout_send_body(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.send_body = v;
        self
    }

    /// Max duration for receiving the response headers, but not the body
    ///
    /// Defaults to `None`.
    pub fn timeout_recv_response(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.recv_response = v;
        self
    }

    /// Max duration for receving the response body.
    ///
    /// Defaults to `None`.
    pub fn timeout_recv_body(mut self, v: Option<Duration>) -> Self {
        self.config().timeouts.recv_body = v;
        self
    }
}

/// Possible config values for headers.
///
/// * `None` no automatic header
/// * `Default` default behavior. I.e. for user-agent something like `ureq/3.1.2`
/// * `Provided` is a user provided header
#[derive(Debug, Clone)]
pub enum AutoHeaderValue {
    /// No automatic header.
    None,

    /// Default behavior.
    ///
    /// I.e. for user-agent something like `ureq/3.1.2`.
    Default,

    /// User provided header value.
    Provided(Arc<String>),
}

impl Default for AutoHeaderValue {
    fn default() -> Self {
        Self::Default
    }
}

impl AutoHeaderValue {
    pub(crate) fn as_str(&self, default: &'static str) -> Option<&str> {
        let x = match self {
            AutoHeaderValue::None => "",
            AutoHeaderValue::Default => default,
            AutoHeaderValue::Provided(v) => v.as_str(),
        };

        if x.is_empty() {
            None
        } else {
            Some(x)
        }
    }
}

impl ConfigBuilder<AgentScope> {
    /// Finalize the config
    pub fn build(self) -> Config {
        self.0 .0
    }
}

impl<Any> ConfigBuilder<RequestScope<Any>> {
    /// Finalize the config
    pub fn build(self) -> RequestBuilder<Any> {
        self.0 .0
    }
}

impl<S: AsSendBody> ConfigBuilder<HttpCrateScope<S>> {
    /// Finalize the config
    pub fn build(self) -> http::Request<S> {
        self.0 .0
    }
}

/// Request timeout configuration.
///
/// This can be configured both on Agent level as well as per request.
#[derive(Clone, Copy)]
pub struct Timeouts {
    /// Timeout for the entire call
    pub global: Option<Duration>,

    /// Timeout for call-by-call when following redirects
    pub per_call: Option<Duration>,

    /// Max duration for doing the DNS lookup when establishing the connection
    pub resolve: Option<Duration>,

    /// Max duration for establishing the connection
    pub connect: Option<Duration>,

    /// Max duration for sending the request, but not the request body.
    pub send_request: Option<Duration>,

    /// Max duration for awaiting a 100-continue response.
    pub await_100: Option<Duration>,

    /// Max duration for sending a request body (if there is one)
    pub send_body: Option<Duration>,

    /// Max duration for receiving the response headers, but not the body
    pub recv_response: Option<Duration>,

    /// Max duration for receving the response body.
    pub recv_body: Option<Duration>,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestLevelConfig(pub Config);

pub(crate) static DEFAULT_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

impl Default for Config {
    fn default() -> Self {
        Self {
            http_status_as_error: true,
            https_only: false,
            ip_family: IpFamily::Any,
            #[cfg(feature = "_tls")]
            tls_config: TlsConfig::default(),
            proxy: Proxy::try_from_env(),
            no_delay: true,
            max_redirects: 10,
            redirect_auth_headers: RedirectAuthHeaders::Never,
            user_agent: AutoHeaderValue::default(),
            accept: AutoHeaderValue::default(),
            accept_encoding: AutoHeaderValue::default(),
            timeouts: Timeouts::default(),
            max_response_header_size: 64 * 1024,
            input_buffer_size: 128 * 1024,
            output_buffer_size: 128 * 1024,
            max_idle_connections: 10,
            max_idle_connections_per_host: 3,
            max_idle_age: Duration::from_secs(15),
            middleware: MiddlewareChain::default(),
            force_send_body: false,
        }
    }
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            global: None,
            per_call: None,
            resolve: None,
            connect: None,
            send_request: None,
            await_100: Some(Duration::from_secs(1)),
            send_body: None,
            recv_response: None,
            recv_body: None,
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut dbg = f.debug_struct("Config");

        dbg.field("http_status_as_error", &self.http_status_as_error)
            .field("https_only", &self.https_only)
            .field("ip_family", &self.ip_family)
            .field("proxy", &self.proxy)
            .field("no_delay", &self.no_delay)
            .field("max_redirects", &self.max_redirects)
            .field("redirect_auth_headers", &self.redirect_auth_headers)
            .field("user_agent", &self.user_agent)
            .field("timeouts", &self.timeouts)
            .field("max_response_header_size", &self.max_response_header_size)
            .field("input_buffer_size", &self.input_buffer_size)
            .field("output_buffer_size", &self.output_buffer_size)
            .field("max_idle_connections", &self.max_idle_connections)
            .field(
                "max_idle_connections_per_host",
                &self.max_idle_connections_per_host,
            )
            .field("max_idle_age", &self.max_idle_age)
            .field("middleware", &self.middleware);

        #[cfg(feature = "_tls")]
        {
            dbg.field("tls_config", &self.tls_config);
        }

        dbg.finish()
    }
}

impl fmt::Debug for Timeouts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timeouts")
            .field("global", &self.global)
            .field("per_call", &self.per_call)
            .field("resolve", &self.resolve)
            .field("connect", &self.connect)
            .field("send_request", &self.send_request)
            .field("await_100", &self.await_100)
            .field("send_body", &self.send_body)
            .field("recv_response", &self.recv_response)
            .field("recv_body", &self.recv_body)
            .finish()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_no_alloc::*;

    #[test]
    fn default_config_clone_does_not_allocate() {
        let c = Config::default();
        assert_no_alloc(|| c.clone());
    }
}
