//! Peer authorization, per-peer budgets, and connection bounds for the Web UI.
//!
//! Every request crosses these gates before a route handler exists to read
//! Quill data, so a refusal is `403` with an empty body on assets, pages, and
//! invokes alike — "not a partial render" is `status == 403 && body.is_empty()`
//! (`specs/029-web-ui-server.md#api--interface-changes`).
//!
//! Client identity is the accepted socket's address and nothing else. The
//! `Host` header is attacker-controlled and names the destination rather than
//! the caller, and reverse DNS is not client authentication, so neither is
//! consulted. Hostname allowlist entries are forward-resolved when a
//! configuration is committed and pinned until the next restart or re-save; an
//! entry that cannot be resolved contributes no address and therefore admits
//! nobody. Loopback bypasses host filtering only — never the session check.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::{ConnectInfo, DefaultBodyLimit, Request, State, connect_info::Connected},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    serve::{IncomingStream, Listener},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Sleep,
};

use crate::web_server::{SESSION_COOKIE_NAME, WebServerState, WebUiConfig};

/// Live browser connections the listener will hold at once. Accepting is
/// suspended past this, so a hostile client cannot exhaust sockets or tasks.
///
/// A browser opens about six parallel connections per origin and keeps them
/// alive, so the previous bound of 8 was under two devices: one phone loading
/// the monitor alongside a desktop browser filled it, accepting stopped, and
/// every later request queued unanswered. The bound exists to cap a hostile
/// client, not to ration honest ones.
pub const MAX_CONCURRENT_CONNECTIONS: usize = 64;
/// How long a connection may sit without reading or writing before it is
/// dropped and its slot returned.
///
/// Without this the slot is held for the connection's lifetime, and a lifetime
/// has no upper bound: an idle keep-alive holds one indefinitely, and a
/// half-closed socket the peer abandoned holds one forever. `REQUEST_TIMEOUT`
/// does not help — it bounds a request, and an idle connection has none. The
/// window clears the monitor's 55-second poll, so a live viewer is never cut
/// off mid-cadence.
const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(90);
/// The largest request body any web route may receive.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// Wall-clock bound on one request, gates included.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

const RATE_WINDOW: Duration = Duration::from_secs(60);
const MAX_GENERAL_REQUESTS: usize = 120;
/// Pairing is the one route class that can turn an unpaired peer into a paired
/// one, so it gets its own far smaller window on top of the general budget.
const MAX_PAIRING_REQUESTS: usize = 10;
/// An unbounded peer table is itself the denial-of-service, so tracking is
/// capped and the least recently active peer is evicted at the cap.
const MAX_TRACKED_PEERS: usize = 256;

/// Names the listener answers to without being listed, so the desktop's own
/// link keeps working whatever the user has configured. An empty allowlist is
/// therefore loopback-only rather than broken.
const IMPLICIT_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// The request classes that carry separate per-peer budgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestClass {
    General,
    Pairing,
}

/// The accepted socket's remote address: the only client identity the gates
/// trust, delivered by Axum's `ConnectInfo` rather than by any header. The
/// address stays private so nothing outside this module can mint a peer.
#[derive(Clone, Copy, Debug)]
pub struct WebPeer(SocketAddr);

/// Host filtering and per-peer budgets for one running listener.
///
/// The default admits only [`IMPLICIT_HOSTS`], so a listener that has not yet
/// adopted a configuration serves this machine and nothing else.
#[derive(Default)]
pub struct RequestGates {
    hosts: RwLock<Vec<String>>,
    peers: Mutex<HashMap<IpAddr, PeerBudget>>,
}

struct PeerBudget {
    general: VecDeque<Instant>,
    pairing: VecDeque<Instant>,
    last_seen: Instant,
}

impl RequestGates {
    /// Adopt a committed configuration's allowed host names. Called on the
    /// config the controller has persisted, so a failed transition keeps the
    /// previous set.
    pub fn adopt_allowlist(&self, config: &WebUiConfig) {
        *self.hosts.write().expect("web gate host lock") = config.allowlist.clone();
    }

    /// Whether the listener answers to the name this request asked for.
    ///
    /// This is a name check, not access control: it is what stops a hostile
    /// domain resolved to this address from reaching the listener (DNS
    /// rebinding). Deciding *who* may connect is the pairing credential's job.
    pub fn allows_host(&self, host: &str) -> bool {
        let Some(name) = host_name(host) else {
            return false;
        };
        if IMPLICIT_HOSTS
            .iter()
            .any(|implicit| name.eq_ignore_ascii_case(implicit))
        {
            return true;
        }
        self.hosts
            .read()
            .expect("web gate host lock")
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name))
    }

    /// Charge one request of `class` to `peer` and report whether it fits the
    /// peer's sliding-window budget.
    pub fn admits_request(&self, peer: IpAddr, class: RequestClass) -> bool {
        let now = Instant::now();
        let cutoff = now - RATE_WINDOW;
        let mut peers = self.peers.lock().expect("web gate peer lock");

        peers.retain(|_, budget| budget.prune(cutoff));
        if peers.len() >= MAX_TRACKED_PEERS && !peers.contains_key(&peer) {
            let evicted = peers
                .iter()
                .min_by_key(|(_, budget)| budget.last_seen)
                .map(|(address, _)| *address);
            if let Some(evicted) = evicted {
                peers.remove(&evicted);
            }
        }

        let budget = peers.entry(peer).or_insert_with(|| PeerBudget::new(now));
        budget.last_seen = now;
        let (window, max) = match class {
            RequestClass::General => (&mut budget.general, MAX_GENERAL_REQUESTS),
            RequestClass::Pairing => (&mut budget.pairing, MAX_PAIRING_REQUESTS),
        };
        if window.len() >= max {
            return false;
        }
        window.push_back(now);
        true
    }

    #[cfg(test)]
    fn tracked_peers(&self) -> usize {
        self.peers.lock().expect("web gate peer lock").len()
    }
}

impl PeerBudget {
    fn new(now: Instant) -> Self {
        Self {
            general: VecDeque::new(),
            pairing: VecDeque::new(),
            last_seen: now,
        }
    }

    /// Drop expired timestamps and report whether the peer is still active.
    fn prune(&mut self, cutoff: Instant) -> bool {
        for window in [&mut self.general, &mut self.pairing] {
            while window.front().is_some_and(|charged| *charged < cutoff) {
                window.pop_front();
            }
        }
        !self.general.is_empty() || !self.pairing.is_empty()
    }
}

/// The name half of a `Host` header, lowercased for comparison.
///
/// A `Host` carries `name[:port]`, and an IPv6 literal keeps its brackets, so
/// the port is stripped from the last colon only when no bracket follows it.
/// The port is deliberately ignored: it is the socket's business, and a name
/// does not become a different name on another port.
fn host_name(host: &str) -> Option<&str> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let name = match host.rfind(']') {
        Some(bracket) => &host[..=bracket],
        None => match host.rfind(':') {
            Some(colon) => &host[..colon],
            None => host,
        },
    };
    (!name.is_empty()).then_some(name)
}

/// Wrap every route — including the fallback — in the gates that apply to all
/// three request classes. Per-class middleware is added by the routes
/// themselves; this is the floor no route can opt out of.
pub fn apply_request_gates(router: Router, state: Arc<WebServerState>) -> Router {
    router
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state, enforce_peer_gate))
        .layer(middleware::from_fn(enforce_request_timeout))
}

/// The general class gate: an allowed host name, a known peer, and that peer
/// inside its general budget. Runs before routing, so a refusal reads no Quill
/// data. Private because [`apply_request_gates`] is its only mount point —
/// mounting it twice would charge one request to the budget twice.
async fn enforce_peer_gate(
    State(state): State<Arc<WebServerState>>,
    request: Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !state.gates.allows_host(host) {
        return refused();
    }
    let Some(peer) = peer_ip(&request) else {
        return refused();
    };
    if !state.gates.admits_request(peer, RequestClass::General) {
        return refused();
    }
    next.run(request).await
}

/// The pairing class's extra budget, mounted by the pairing route on top of
/// the general gate.
pub async fn enforce_pairing_budget(
    State(state): State<Arc<WebServerState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(peer) = peer_ip(&request) else {
        return refused();
    };
    if !state.gates.admits_request(peer, RequestClass::Pairing) {
        return refused();
    }
    next.run(request).await
}

/// The authenticated class's session check, mounted by the routes that serve
/// Quill data. Loopback gets no exemption here.
pub async fn require_session(request: Request, next: Next) -> Response {
    if session_is_live(&request) {
        next.run(request).await
    } else {
        refused()
    }
}

async fn enforce_request_timeout(request: Request, next: Next) -> Response {
    match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

fn peer_ip(request: &Request) -> Option<IpAddr> {
    request
        .extensions()
        .get::<ConnectInfo<WebPeer>>()
        .map(|ConnectInfo(peer)| peer.0.ip().to_canonical())
}

pub(super) fn session_is_live(request: &Request) -> bool {
    request
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| name.trim() == SESSION_COOKIE_NAME)
        .is_some_and(|(_, token)| crate::web_pairing::verify_session(token.trim()))
}

/// The one refusal shape, shared with the routes: no body, no header, nothing
/// read.
pub(super) fn refused() -> Response {
    StatusCode::FORBIDDEN.into_response()
}

/// A [`TcpListener`] that stops accepting past [`MAX_CONCURRENT_CONNECTIONS`].
///
/// The permit is taken before `accept` and released when the connection's IO is
/// dropped, so the bound counts live connections rather than in-flight
/// requests. [`IDLE_CONNECTION_TIMEOUT`] is what keeps that lifetime finite.
pub struct BoundedListener {
    inner: TcpListener,
    permits: Arc<Semaphore>,
}

/// An accepted connection holding its concurrency slot until it goes idle.
pub struct BoundedConnection {
    stream: TcpStream,
    idle: Pin<Box<Sleep>>,
    _permit: OwnedSemaphorePermit,
}

impl BoundedListener {
    pub fn new(inner: TcpListener) -> Self {
        Self {
            inner,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS)),
        }
    }
}

impl Listener for BoundedListener {
    type Io = BoundedConnection;
    type Addr = WebPeer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let permit = Arc::clone(&self.permits)
                .acquire_owned()
                .await
                .expect("web connection semaphore is never closed");
            match self.inner.accept().await {
                Ok((stream, address)) => {
                    return (
                        BoundedConnection {
                            stream,
                            idle: Box::pin(tokio::time::sleep(IDLE_CONNECTION_TIMEOUT)),
                            _permit: permit,
                        },
                        WebPeer(address),
                    );
                }
                Err(error) => {
                    drop(permit);
                    handle_accept_error(error).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr().map(WebPeer)
    }
}

async fn handle_accept_error(error: io::Error) {
    if matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    ) {
        return;
    }
    // Descriptor and buffer exhaustion re-fire instantly, so back off rather
    // than spin the accept loop while the condition clears.
    log::warn!("Web UI listener could not accept a connection: {error}");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

impl Connected<IncomingStream<'_, BoundedListener>> for WebPeer {
    fn connect_info(stream: IncomingStream<'_, BoundedListener>) -> Self {
        *stream.remote_addr()
    }
}

impl BoundedConnection {
    /// Report whether the connection has gone idle, and otherwise arm the timer
    /// to fire once more from now. Called on every completed read and write, so
    /// the deadline tracks activity rather than connection age.
    fn idle_expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.idle.as_mut().poll(cx).is_ready()
    }

    fn mark_active(&mut self) {
        self.idle
            .as_mut()
            .reset(tokio::time::Instant::now() + IDLE_CONNECTION_TIMEOUT);
    }
}

fn idle_timeout_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "web connection idle past its timeout",
    )
}

impl AsyncRead for BoundedConnection {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.idle_expired(cx) {
            return Poll::Ready(Err(idle_timeout_error()));
        }
        let polled = Pin::new(&mut this.stream).poll_read(cx, buf);
        if polled.is_ready() {
            this.mark_active();
        }
        polled
    }
}

impl AsyncWrite for BoundedConnection {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.idle_expired(cx) {
            return Poll::Ready(Err(idle_timeout_error()));
        }
        let polled = Pin::new(&mut this.stream).poll_write(cx, buf);
        if polled.is_ready() {
            this.mark_active();
        }
        polled
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Bytes,
        routing::{get, post},
    };
    use std::net::Ipv4Addr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const QUILL_DATA: &str = "project /home/dev/quill 12345 tokens";
    const PAIR_PAGE: &str = "pairing page";

    fn ip(literal: &str) -> IpAddr {
        literal.parse().expect("test address")
    }

    async fn gates_answering_to(allowlist: &[&str]) -> Arc<WebServerState> {
        let state = Arc::new(WebServerState::default());
        state.gates.adopt_allowlist(&WebUiConfig {
            enabled: true,
            port: 19878,
            allowlist: allowlist.iter().map(|entry| (*entry).to_string()).collect(),
        });
        state
    }

    /// The three request classes of
    /// `specs/029-web-ui-server.md#architecture-approach`, with handlers that
    /// would leak Quill data if a gate let a request through. The POST stubs
    /// read their body because the size cap is enforced at extraction.
    fn class_router(state: &Arc<WebServerState>) -> Router {
        let public = Router::new()
            .route("/pair", get(|| async { PAIR_PAGE }))
            .route(
                "/api/web/pair",
                post(|_: Bytes| async { StatusCode::NO_CONTENT }).layer(
                    middleware::from_fn_with_state(Arc::clone(state), enforce_pairing_budget),
                ),
            );
        // The entry document sits behind the same session check but answers a
        // navigation with a redirect, so it is modelled as its own class.
        let document = Router::new()
            .route("/", get(|| async { QUILL_DATA }))
            .layer(middleware::from_fn(
                crate::web_server::router::pair_or_document,
            ));
        let authenticated = Router::new()
            .route("/assets/app.js", get(|| async { QUILL_DATA }))
            .route("/api/web/invoke", post(|_: Bytes| async { QUILL_DATA }))
            .layer(middleware::from_fn(require_session));

        apply_request_gates(
            public.merge(document).merge(authenticated),
            Arc::clone(state),
        )
    }

    /// Serve the class router while presenting `peer` as the accepted socket's
    /// address, which a loopback test socket cannot otherwise be.
    async fn spawn_with_peer(state: &Arc<WebServerState>, peer: IpAddr) -> String {
        let app = class_router(state).layer(middleware::from_fn(
            move |mut request: Request, next: Next| async move {
                request
                    .extensions_mut()
                    .insert(ConnectInfo(WebPeer(SocketAddr::new(peer, 51000))));
                next.run(request).await
            },
        ));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind gate test listener");
        let address = listener.local_addr().expect("gate test address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve gate test");
        });
        format!("http://{address}")
    }

    async fn assert_all_classes_refused(base: &str, host: &str) {
        let client = reqwest::Client::new();
        for (method, path) in [
            (reqwest::Method::GET, "/pair"),
            (reqwest::Method::POST, "/api/web/pair"),
            (reqwest::Method::GET, "/"),
            (reqwest::Method::GET, "/assets/app.js"),
            (reqwest::Method::POST, "/api/web/invoke"),
            (reqwest::Method::GET, "/does-not-exist"),
        ] {
            let response = client
                .request(method.clone(), format!("{base}{path}"))
                .header(header::HOST, host)
                .send()
                .await
                .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{method} {path} as {host:?}"
            );
            assert!(
                response.bytes().await.expect("refusal body").is_empty(),
                "{method} {path} must refuse before reading any Quill data"
            );
        }
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Host denial precedes any data]]
    #[tokio::test]
    async fn an_unlisted_host_is_refused_on_every_route_class() {
        let state = gates_answering_to(&["quill.lan", "192.168.1.7"]).await;
        let base = spawn_with_peer(&state, ip("203.0.113.9")).await;

        // A name the listener does not answer to is refused before routing,
        // whatever address it arrives from. This is the rebinding defence: an
        // attacker's domain pointed at this address gets nothing.
        for host in ["evil.example", "quill.lan.evil.example", "10.0.1.9", ""] {
            assert_all_classes_refused(&base, host).await;
        }

        // Listed names answer, port and case notwithstanding — a name is not a
        // different name on another port.
        for allowed in ["quill.lan", "QUILL.LAN", "quill.lan:19878", "192.168.1.7"] {
            let response = reqwest::Client::new()
                .get(format!("{base}/pair"))
                .header(header::HOST, allowed)
                .send()
                .await
                .expect("pair page");
            assert_eq!(response.status(), StatusCode::OK, "{allowed}");
            assert_eq!(response.text().await.expect("pair body"), PAIR_PAGE);
        }
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Host denial precedes any data]]
    #[tokio::test]
    async fn an_empty_allowlist_answers_only_to_local_names() {
        let state = gates_answering_to(&[]).await;
        let base = spawn_with_peer(&state, ip("203.0.113.9")).await;
        assert_all_classes_refused(&base, "quill.lan").await;

        // The implicit local names always answer, so an empty list is
        // loopback-only rather than a listener that refuses its own UI. They
        // still face the session gate on authenticated routes.
        let client = reqwest::Client::new();
        assert_eq!(
            client
                .get(format!("{base}/pair"))
                .header(header::HOST, "localhost")
                .send()
                .await
                .expect("pair page")
                .status(),
            StatusCode::OK
        );
        let response = client
            .get(format!("{base}/assets/app.js"))
            .header(header::HOST, "127.0.0.1:19878")
            .send()
            .await
            .expect("unpaired local request");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(response.bytes().await.expect("refusal body").is_empty());

        // The document redirects instead of refusing, and still hands over no
        // bundle content.
        let document = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("non-redirecting client")
            .get(format!("{base}/"))
            .header(header::HOST, "localhost")
            .send()
            .await
            .expect("unpaired local document");
        assert_eq!(document.status(), StatusCode::SEE_OTHER);
        assert!(document.bytes().await.expect("redirect body").is_empty());
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Host denial precedes any data]]
    #[tokio::test]
    async fn only_listed_and_implicit_names_are_answered() {
        let state = gates_answering_to(&["quill.lan"]).await;
        let gates = &state.gates;

        for implicit in ["localhost", "127.0.0.1", "127.0.0.1:19878", "[::1]:19878"] {
            assert!(gates.allows_host(implicit), "{implicit}");
        }
        assert!(gates.allows_host("quill.lan"));
        assert!(gates.allows_host("quill.lan:19878"));

        // No suffix, prefix, or empty match, and nothing resolves: the entry is
        // compared as a name, so this machine's DNS cannot widen it.
        for refused in [
            "",
            "lan",
            "quill",
            "notquill.lan",
            "quill.lan.evil",
            "192.168.1.7",
        ] {
            assert!(!gates.allows_host(refused), "{refused}");
        }
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Request classes carry bounded per-peer budgets]]
    #[tokio::test]
    async fn per_peer_budgets_are_bounded_isolated_and_capped() {
        let gates = RequestGates::default();
        let peer = ip("10.0.0.9");
        let neighbour = ip("10.0.0.10");

        for attempt in 0..MAX_GENERAL_REQUESTS {
            assert!(
                gates.admits_request(peer, RequestClass::General),
                "{attempt}"
            );
        }
        assert!(!gates.admits_request(peer, RequestClass::General));

        // The pairing class is stricter and charged separately, so exhausting
        // one class cannot borrow or grant capacity in the other.
        for attempt in 0..MAX_PAIRING_REQUESTS {
            assert!(
                gates.admits_request(peer, RequestClass::Pairing),
                "{attempt}"
            );
        }
        assert!(!gates.admits_request(peer, RequestClass::Pairing));
        assert!(gates.admits_request(neighbour, RequestClass::General));

        for index in 0..MAX_TRACKED_PEERS + 64 {
            let filler = IpAddr::V4(Ipv4Addr::from(0x0b00_0000 + index as u32));
            gates.admits_request(filler, RequestClass::General);
        }
        assert!(gates.tracked_peers() <= MAX_TRACKED_PEERS);
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Connection and body caps bound one client]]
    #[tokio::test]
    async fn the_listener_holds_no_more_than_the_connection_cap() {
        let state = gates_answering_to(&[]).await;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind bounded listener");
        let address = listener.local_addr().expect("bounded listener address");
        let app = class_router(&state);
        tokio::spawn(async move {
            axum::serve(
                BoundedListener::new(listener),
                app.into_make_service_with_connect_info::<WebPeer>(),
            )
            .await
            .expect("serve bounded listener");
        });

        async fn ask(stream: &mut TcpStream, wait: Duration) -> Option<String> {
            stream
                // An implicit local name, so the cap is what this test
                // measures rather than the host gate in front of it.
                .write_all(b"GET /pair HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .ok()?;
            let mut buffer = [0u8; 128];
            let read = tokio::time::timeout(wait, stream.read(&mut buffer))
                .await
                .ok()?
                .ok()?;
            Some(String::from_utf8_lossy(&buffer[..read]).into_owned())
        }

        let mut held = Vec::new();
        for slot in 0..MAX_CONCURRENT_CONNECTIONS {
            let mut stream = TcpStream::connect(address).await.expect("hold connection");
            let answer = ask(&mut stream, Duration::from_secs(5))
                .await
                .unwrap_or_else(|| panic!("connection {slot} was not served"));
            assert!(answer.contains("200 OK"), "connection {slot}: {answer}");
            held.push(stream);
        }

        let mut queued = TcpStream::connect(address)
            .await
            .expect("queued connection");
        assert!(
            ask(&mut queued, Duration::from_millis(400)).await.is_none(),
            "a connection past the cap must not be served while the cap is full"
        );

        held.pop();
        let mut buffer = [0u8; 128];
        let read = tokio::time::timeout(Duration::from_secs(5), queued.read(&mut buffer))
            .await
            .expect("freed slot must serve the queued connection")
            .expect("queued response");
        assert!(String::from_utf8_lossy(&buffer[..read]).contains("200 OK"));
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Connection and body caps bound one client]]
    #[tokio::test]
    async fn an_oversized_body_is_rejected_at_the_size_cap() {
        let state = gates_answering_to(&[]).await;
        let base = spawn_with_peer(&state, ip("10.0.0.9")).await;

        let response = reqwest::Client::new()
            .post(format!("{base}/api/web/pair"))
            .body(vec![b'x'; MAX_BODY_BYTES + 1])
            .send()
            .await
            .expect("oversized pairing request");

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }
}
