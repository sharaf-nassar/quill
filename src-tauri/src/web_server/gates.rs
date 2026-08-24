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
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
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
};

use crate::web_server::{SESSION_COOKIE_NAME, WebServerState, WebUiConfig, WebUiHostPolicy};

/// Live browser connections the listener will hold at once. Accepting is
/// suspended past this, so a hostile client cannot exhaust sockets or tasks.
pub const MAX_CONCURRENT_CONNECTIONS: usize = 8;
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
/// One budget shared by every hostname entry, because the controller pins
/// under its transition lock: a hostile or unreachable resolver must not be
/// able to stall a settings save once per allowlist entry.
const RESOLVE_BUDGET: Duration = Duration::from_secs(5);

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
/// The default is deny-all, so a listener that has not yet pinned a
/// configuration admits nobody but loopback.
#[derive(Default)]
pub struct RequestGates {
    policy: RwLock<PinnedPolicy>,
    peers: Mutex<HashMap<IpAddr, PeerBudget>>,
}

#[derive(Default)]
struct PinnedPolicy {
    accept_all: bool,
    networks: Vec<PinnedNetwork>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PinnedNetwork {
    base: IpAddr,
    prefix: u8,
}

struct PeerBudget {
    general: VecDeque<Instant>,
    pairing: VecDeque<Instant>,
    last_seen: Instant,
}

impl RequestGates {
    /// Adopt a committed configuration: resolve its hostname entries once and
    /// pin the resulting addresses. Called on the config the controller has
    /// persisted, so a failed transition keeps the previous pinned set.
    pub async fn pin_allowlist(&self, config: &WebUiConfig) {
        *self.policy.write().expect("web gate policy lock") = PinnedPolicy {
            accept_all: matches!(config.host_policy, WebUiHostPolicy::All),
            networks: pin_networks(&config.allowlist).await,
        };
    }

    /// Whether the host policy admits this peer. Loopback always passes here
    /// and still faces the session gate on authenticated routes.
    pub fn allows_peer(&self, peer: IpAddr) -> bool {
        if peer.is_loopback() {
            return true;
        }
        let policy = self.policy.read().expect("web gate policy lock");
        policy.accept_all || policy.networks.iter().any(|network| network.contains(peer))
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

impl PinnedNetwork {
    fn host(address: IpAddr) -> Self {
        let prefix = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        Self {
            base: address,
            prefix,
        }
    }

    /// Entries reach here already canonicalized by
    /// [[src-tauri/src/web_config.rs#canonical_allowlist_entry]]; anything that
    /// is not an address or network is a hostname needing resolution.
    fn parse_literal(entry: &str) -> Option<Self> {
        match entry.split_once('/') {
            Some((address, prefix)) => Some(Self {
                base: address.parse().ok()?,
                prefix: prefix.parse().ok()?,
            }),
            None => Some(Self::host(entry.parse().ok()?)),
        }
    }

    fn contains(&self, peer: IpAddr) -> bool {
        match (self.base, peer) {
            (IpAddr::V4(base), IpAddr::V4(peer)) => {
                mask_v4(base, self.prefix) == mask_v4(peer, self.prefix)
            }
            (IpAddr::V6(base), IpAddr::V6(peer)) => {
                mask_v6(base, self.prefix) == mask_v6(peer, self.prefix)
            }
            _ => false,
        }
    }
}

fn mask_v4(address: Ipv4Addr, prefix: u8) -> u32 {
    let bits = u32::from(address);
    if prefix >= 32 {
        bits
    } else {
        bits & (u32::MAX << (32 - prefix))
    }
}

fn mask_v6(address: Ipv6Addr, prefix: u8) -> u128 {
    let bits = u128::from(address);
    if prefix >= 128 {
        bits
    } else {
        bits & (u128::MAX << (128 - prefix))
    }
}

/// Turn canonical allowlist entries into the addresses they admit. Literals
/// resolve for free; hostnames share one forward-resolution deadline. Failure,
/// an empty answer, and an exhausted budget all pin nothing, which denies the
/// entry until the next re-save.
async fn pin_networks(entries: &[String]) -> Vec<PinnedNetwork> {
    let mut networks = Vec::new();
    let mut hostnames = Vec::new();
    for entry in entries {
        match PinnedNetwork::parse_literal(entry) {
            Some(network) => networks.push(network),
            None => hostnames.push(entry.as_str()),
        }
    }

    let deadline = tokio::time::Instant::now() + RESOLVE_BUDGET;
    for hostname in hostnames {
        let Ok(resolved) =
            tokio::time::timeout_at(deadline, tokio::net::lookup_host((hostname, 0))).await
        else {
            log::warn!(
                "Web UI allowlist resolution exceeded {RESOLVE_BUDGET:?}; {hostname} and any later hostname entry admit nobody until the next save"
            );
            break;
        };
        match resolved {
            Ok(addresses) => {
                networks.extend(addresses.map(|address| PinnedNetwork::host(address.ip())));
            }
            Err(error) => {
                log::warn!("Web UI allowlist entry {hostname} did not resolve: {error}");
            }
        }
    }
    networks
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

/// The general class gate: known peer, allowed by the host policy, inside its
/// general budget. Runs before routing, so a refusal reads no Quill data.
/// Private because [`apply_request_gates`] is its only mount point — mounting
/// it twice would charge one request to the budget twice.
async fn enforce_peer_gate(
    State(state): State<Arc<WebServerState>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(peer) = peer_ip(&request) else {
        return refused();
    };
    if !state.gates.allows_peer(peer) {
        return refused();
    }
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

fn session_is_live(request: &Request) -> bool {
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
/// requests: an idle keep-alive socket still costs its slot.
pub struct BoundedListener {
    inner: TcpListener,
    permits: Arc<Semaphore>,
}

/// An accepted connection holding its concurrency slot for its whole lifetime.
pub struct BoundedConnection {
    stream: TcpStream,
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

impl AsyncRead for BoundedConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for BoundedConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const QUILL_DATA: &str = "project /home/dev/quill 12345 tokens";
    const PAIR_PAGE: &str = "pairing page";

    fn ip(literal: &str) -> IpAddr {
        literal.parse().expect("test address")
    }

    async fn gates_pinned_to(
        allowlist: &[&str],
        host_policy: WebUiHostPolicy,
    ) -> Arc<WebServerState> {
        let state = Arc::new(WebServerState::default());
        state
            .gates
            .pin_allowlist(&WebUiConfig {
                enabled: true,
                port: 19878,
                host_policy,
                allowlist: allowlist.iter().map(|entry| (*entry).to_string()).collect(),
            })
            .await;
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
        let authenticated = Router::new()
            .route("/", get(|| async { QUILL_DATA }))
            .route("/assets/app.js", get(|| async { QUILL_DATA }))
            .route("/api/web/invoke", post(|_: Bytes| async { QUILL_DATA }))
            .layer(middleware::from_fn(require_session));

        apply_request_gates(public.merge(authenticated), Arc::clone(state))
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

    async fn assert_all_classes_refused(base: &str) {
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
                .send()
                .await
                .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {path}");
            assert!(
                response.bytes().await.expect("refusal body").is_empty(),
                "{method} {path} must refuse before reading any Quill data"
            );
        }
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Host denial precedes any data]]
    #[tokio::test]
    async fn a_non_allowed_peer_is_refused_on_every_route_class() {
        let state =
            gates_pinned_to(&["10.0.0.0/24", "192.168.1.7"], WebUiHostPolicy::Allowlist).await;

        assert_all_classes_refused(&spawn_with_peer(&state, ip("203.0.113.9")).await).await;
        assert_all_classes_refused(&spawn_with_peer(&state, ip("10.0.1.9")).await).await;
        assert_all_classes_refused(&spawn_with_peer(&state, ip("192.168.1.8")).await).await;

        for allowed in ["10.0.0.9", "192.168.1.7"] {
            let base = spawn_with_peer(&state, ip(allowed)).await;
            let response = reqwest::get(format!("{base}/pair"))
                .await
                .expect("pair page");
            assert_eq!(response.status(), StatusCode::OK, "{allowed}");
            assert_eq!(response.text().await.expect("pair body"), PAIR_PAGE);
        }
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Host denial precedes any data]]
    #[tokio::test]
    async fn an_empty_or_unresolvable_allowlist_admits_nobody_but_loopback() {
        for allowlist in [vec![], vec!["nonexistent-quill-host.invalid"]] {
            let state = gates_pinned_to(&allowlist, WebUiHostPolicy::Allowlist).await;
            assert_all_classes_refused(&spawn_with_peer(&state, ip("203.0.113.9")).await).await;

            // Loopback bypasses host filtering only: the public class answers,
            // the authenticated class still demands a session.
            let base = spawn_with_peer(&state, ip("127.0.0.1")).await;
            let client = reqwest::Client::new();
            assert_eq!(
                client
                    .get(format!("{base}/pair"))
                    .send()
                    .await
                    .expect("pair page")
                    .status(),
                StatusCode::OK
            );
            for path in ["/", "/assets/app.js"] {
                let response = client
                    .get(format!("{base}{path}"))
                    .send()
                    .await
                    .expect("unpaired loopback request");
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
                assert!(
                    response.bytes().await.expect("refusal body").is_empty(),
                    "{path}"
                );
            }
        }
    }

    // @lat: [[web-ui-server-tests#Web UI Server Test Specs#Host denial precedes any data]]
    #[tokio::test]
    async fn a_pinned_hostname_admits_only_its_resolved_addresses() {
        let state = gates_pinned_to(&["localhost"], WebUiHostPolicy::Allowlist).await;

        assert!(state.gates.allows_peer(ip("127.0.0.1")));
        assert!(!state.gates.allows_peer(ip("203.0.113.9")));

        // `host_policy=all` is the only setting that skips the pinned set.
        let open = gates_pinned_to(&[], WebUiHostPolicy::All).await;
        assert!(open.gates.allows_peer(ip("203.0.113.9")));
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
        let state = gates_pinned_to(&[], WebUiHostPolicy::Allowlist).await;
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
                .write_all(b"GET /pair HTTP/1.1\r\nHost: quill\r\n\r\n")
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
        let state = gates_pinned_to(&["10.0.0.0/24"], WebUiHostPolicy::Allowlist).await;
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
