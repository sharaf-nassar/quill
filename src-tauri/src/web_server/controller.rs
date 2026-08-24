//! Runtime ownership and recoverable transitions for the Web UI listener.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::Arc,
    time::Duration,
};

use tokio::{
    net::TcpListener,
    sync::{Mutex, oneshot},
    task::JoinHandle,
};

use crate::{
    storage::Storage,
    web_config::{load_web_ui_config, save_web_ui_config, validate_web_ui_config},
    web_server::{
        WEB_UI_LAST_ERROR_KEY, WebServerState, WebUiConfig, WebUiError, WebUiHostPolicy,
        WebUiStatus, format_reachable_urls,
        gates::{BoundedListener, WebPeer},
        router,
    },
};

const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const LOOPBACK_BIND_IP: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const EXTERNAL_BIND_IP: IpAddr = IpAddr::V4(Ipv4Addr::UNSPECIFIED);

#[cfg(test)]
type BindFailureHook = Arc<dyn Fn(SocketAddr) -> Option<std::io::Error> + Send + Sync>;

/// Serializes every configuration transition and owns the process's Web UI
/// socket. Construction is inert; startup loading and binding happen only from
/// [`Self::initialize`], which setup launches as an async task.
pub struct WebListenerController {
    state: Mutex<ControllerState>,
    router_state: Arc<WebServerState>,
    #[cfg(test)]
    bind_failure_hook: std::sync::Mutex<Option<BindFailureHook>>,
}

#[derive(Default)]
struct ControllerState {
    initialized: bool,
    config: WebUiConfig,
    listener: Option<RunningListener>,
    last_error: Option<String>,
    load_error: Option<WebUiError>,
}

struct RunningListener {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl RunningListener {
    async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, &mut self.task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.is_cancelled() => {}
            Ok(Err(error)) => log::warn!("Web UI listener task failed during shutdown: {error}"),
            Err(_) => {
                log::warn!("Web UI listener did not stop gracefully; aborting it");
                self.task.abort();
                let _ = self.task.await;
            }
        }
    }
}

impl WebListenerController {
    pub fn new(router_state: Arc<WebServerState>) -> Self {
        Self {
            state: Mutex::new(ControllerState::default()),
            router_state,
            #[cfg(test)]
            bind_failure_hook: std::sync::Mutex::new(None),
        }
    }

    /// Load durable state and start an enabled listener. The caller must spawn
    /// this future; no filesystem, database, bind, or shutdown work belongs on
    /// Tauri's setup critical path.
    pub async fn initialize(&self, storage: &Storage) {
        let mut state = self.state.lock().await;
        self.ensure_initialized(&mut state, storage).await;
    }

    pub async fn config(&self, storage: &Storage) -> Result<WebUiConfig, WebUiError> {
        let mut state = self.state.lock().await;
        self.ensure_initialized(&mut state, storage).await;
        self.discard_finished_listener(&mut state);
        if let Some(error) = state.load_error.clone() {
            return Err(error);
        }
        Ok(state.config.clone())
    }

    pub async fn status(&self, storage: &Storage) -> WebUiStatus {
        let mut state = self.state.lock().await;
        self.ensure_initialized(&mut state, storage).await;
        self.discard_finished_listener(&mut state);

        let Some(listener) = state.listener.as_ref() else {
            return WebUiStatus {
                running: false,
                bound_addr: None,
                reachable_urls: Vec::new(),
                last_error: state.last_error.clone(),
            };
        };

        WebUiStatus {
            running: true,
            bound_addr: Some(listener.addr.to_string()),
            reachable_urls: format_reachable_urls(
                concrete_listener_addresses(listener.addr.ip()),
                listener.addr.port(),
            ),
            last_error: state.last_error.clone(),
        }
    }

    /// Apply one validated configuration while preserving the durable and live
    /// last-known-good state on every failed transition.
    pub async fn apply_config(
        &self,
        storage: &Storage,
        candidate: WebUiConfig,
    ) -> Result<WebUiConfig, WebUiError> {
        let candidate = validate_web_ui_config(candidate).map_err(WebUiError::from)?;
        let mut state = self.state.lock().await;
        self.ensure_initialized(&mut state, storage).await;
        self.discard_finished_listener(&mut state);

        if !candidate.enabled {
            return self.disable(&mut state, storage, candidate).await;
        }

        let target = bind_address(&candidate);
        let Some(current) = state.listener.as_ref() else {
            return self.enable(&mut state, storage, candidate, target).await;
        };

        if current.addr == target {
            let saved = self.persist_config(storage, candidate).await?;
            state.config = saved.clone();
            state.load_error = None;
            clear_last_error(storage, &mut state);
            return Ok(saved);
        }

        if current.addr.port() != target.port() {
            return self
                .rebind_different_port(&mut state, storage, candidate, target)
                .await;
        }

        self.rebind_same_port(&mut state, storage, candidate, target)
            .await
    }

    async fn ensure_initialized(&self, state: &mut ControllerState, storage: &Storage) {
        if state.initialized {
            return;
        }

        state.last_error = read_last_error(storage);
        let config = match tokio::task::block_in_place(|| load_web_ui_config(storage)) {
            Ok(config) => config,
            Err(error) => {
                let error = WebUiError::from(error);
                state.last_error = Some(error.message.clone());
                state.load_error = Some(error);
                state.initialized = true;
                return;
            }
        };
        state.config = config.clone();
        self.router_state.gates.pin_allowlist(&config).await;

        if config.enabled {
            let target = bind_address(&config);
            match self.bind(target).await {
                Ok(listener) => {
                    state.listener = Some(listener);
                    clear_last_error(storage, state);
                }
                Err(error) => {
                    state.last_error = Some(error.message.clone());
                    persist_last_error(storage, Some(&error.message));
                }
            }
        }
        state.initialized = true;
    }

    async fn enable(
        &self,
        state: &mut ControllerState,
        storage: &Storage,
        candidate: WebUiConfig,
        target: SocketAddr,
    ) -> Result<WebUiConfig, WebUiError> {
        let listener = match self.bind(target).await {
            Ok(listener) => listener,
            Err(error) => return Err(record_runtime_error(state, error)),
        };
        let saved = match self.persist_config(storage, candidate).await {
            Ok(saved) => saved,
            Err(error) => {
                listener.shutdown().await;
                return Err(record_runtime_error(state, error));
            }
        };
        state.listener = Some(listener);
        state.config = saved.clone();
        state.load_error = None;
        clear_last_error(storage, state);
        Ok(saved)
    }

    async fn disable(
        &self,
        state: &mut ControllerState,
        storage: &Storage,
        candidate: WebUiConfig,
    ) -> Result<WebUiConfig, WebUiError> {
        let saved = self.persist_config(storage, candidate).await?;
        let old = state.listener.take();
        state.config = saved.clone();
        state.load_error = None;
        clear_last_error(storage, state);
        if let Some(listener) = old {
            listener.shutdown().await;
        }
        Ok(saved)
    }

    async fn rebind_different_port(
        &self,
        state: &mut ControllerState,
        storage: &Storage,
        candidate: WebUiConfig,
        target: SocketAddr,
    ) -> Result<WebUiConfig, WebUiError> {
        // Different ports can overlap safely: prove the replacement can bind,
        // persist only after that proof, publish it, then retire the old task.
        let replacement = match self.bind(target).await {
            Ok(listener) => listener,
            Err(error) => return Err(record_runtime_error(state, error)),
        };
        let saved = match self.persist_config(storage, candidate).await {
            Ok(saved) => saved,
            Err(error) => {
                replacement.shutdown().await;
                return Err(record_runtime_error(state, error));
            }
        };
        let old = state.listener.replace(replacement);
        state.config = saved.clone();
        state.load_error = None;
        clear_last_error(storage, state);
        if let Some(listener) = old {
            listener.shutdown().await;
        }
        Ok(saved)
    }

    async fn rebind_same_port(
        &self,
        state: &mut ControllerState,
        storage: &Storage,
        candidate: WebUiConfig,
        target: SocketAddr,
    ) -> Result<WebUiConfig, WebUiError> {
        // A loopback/wildcard change on one port cannot bind-new-first. Stop the
        // old socket, try the candidate, and restore the old bind on any bind or
        // persistence failure before changing the durable config.
        let old = state
            .listener
            .take()
            .expect("same-port transition has listener");
        let old_target = old.addr;
        old.shutdown().await;

        let replacement = match self.bind(target).await {
            Ok(listener) => listener,
            Err(error) => {
                return Err(self.restore_previous(state, old_target, error).await);
            }
        };
        let saved = match self.persist_config(storage, candidate).await {
            Ok(saved) => saved,
            Err(error) => {
                replacement.shutdown().await;
                return Err(self.restore_previous(state, old_target, error).await);
            }
        };

        state.listener = Some(replacement);
        state.config = saved.clone();
        state.load_error = None;
        clear_last_error(storage, state);
        Ok(saved)
    }

    async fn restore_previous(
        &self,
        state: &mut ControllerState,
        old_target: SocketAddr,
        transition_error: WebUiError,
    ) -> WebUiError {
        match self.bind(old_target).await {
            Ok(listener) => {
                state.listener = Some(listener);
                record_runtime_error(state, transition_error)
            }
            Err(rollback_error) => {
                let error = WebUiError::rollback(&transition_error, &rollback_error);
                record_runtime_error(state, error)
            }
        }
    }

    async fn bind(&self, address: SocketAddr) -> Result<RunningListener, WebUiError> {
        #[cfg(test)]
        if let Some(error) = self
            .bind_failure_hook
            .lock()
            .expect("bind failure hook lock")
            .as_ref()
            .and_then(|hook| hook(address))
        {
            return Err(WebUiError::bind(address, &error));
        }

        let listener = TcpListener::bind(address)
            .await
            .map_err(|error| WebUiError::bind(address, &error))?;
        let addr = listener
            .local_addr()
            .map_err(|error| WebUiError::bind(address, &error))?;
        let app = router(Arc::clone(&self.router_state));
        let (shutdown, shutdown_requested) = oneshot::channel();
        let task = tokio::spawn(async move {
            // `ConnectInfo` is the gates' only client identity, and the bounded
            // listener is what caps live connections.
            if let Err(error) = axum::serve(
                BoundedListener::new(listener),
                app.into_make_service_with_connect_info::<WebPeer>(),
            )
            .with_graceful_shutdown(async move {
                let _ = shutdown_requested.await;
            })
            .await
            {
                log::error!("Web UI listener stopped: {error}");
            }
        });
        Ok(RunningListener {
            addr,
            shutdown: Some(shutdown),
            task,
        })
    }

    fn discard_finished_listener(&self, state: &mut ControllerState) {
        if state
            .listener
            .as_ref()
            .is_some_and(|listener| listener.task.is_finished())
        {
            state.listener = None;
            state.last_error = Some("The Web UI listener stopped unexpectedly.".to_string());
        }
    }

    /// Persist a validated candidate and, once it is durable, pin the
    /// addresses its allowlist resolves to. Pinning follows the commit so a
    /// failed transition leaves the previous policy in force.
    async fn persist_config(
        &self,
        storage: &Storage,
        config: WebUiConfig,
    ) -> Result<WebUiConfig, WebUiError> {
        let saved = tokio::task::block_in_place(|| save_web_ui_config(storage, config))
            .map_err(WebUiError::from)?;
        self.router_state.gates.pin_allowlist(&saved).await;
        Ok(saved)
    }

    #[cfg(test)]
    fn set_bind_failure_hook(&self, hook: BindFailureHook) {
        *self
            .bind_failure_hook
            .lock()
            .expect("bind failure hook lock") = Some(hook);
    }
}

fn bind_address(config: &WebUiConfig) -> SocketAddr {
    let bind_ip = if needs_external_bind(config) {
        EXTERNAL_BIND_IP
    } else {
        LOOPBACK_BIND_IP
    };
    SocketAddr::new(bind_ip, config.port)
}

fn needs_external_bind(config: &WebUiConfig) -> bool {
    match config.host_policy {
        WebUiHostPolicy::All => true,
        WebUiHostPolicy::Allowlist => config
            .allowlist
            .iter()
            .any(|entry| !allowlist_entry_is_loopback(entry)),
    }
}

fn allowlist_entry_is_loopback(entry: &str) -> bool {
    if entry.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(address) = entry.parse::<IpAddr>() {
        return address.is_loopback();
    }
    let Some((address, prefix)) = entry.split_once('/') else {
        // Hostnames other than localhost may resolve off-device. The request
        // gate pins their concrete addresses when it applies the same config.
        return false;
    };
    let Ok(address) = address.parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    match address {
        IpAddr::V4(address) => prefix >= 8 && address.octets()[0] == 127,
        IpAddr::V6(address) => prefix == 128 && address.is_loopback(),
    }
}

fn read_last_error(storage: &Storage) -> Option<String> {
    match tokio::task::block_in_place(|| storage.get_setting(WEB_UI_LAST_ERROR_KEY)) {
        Ok(Some(error)) if !error.is_empty() => Some(error),
        Ok(_) => None,
        Err(error) => {
            log::error!("Read Web UI listener error: {error}");
            None
        }
    }
}

fn persist_last_error(storage: &Storage, error: Option<&str>) {
    let result = tokio::task::block_in_place(|| match error {
        Some(error) => storage.set_setting(WEB_UI_LAST_ERROR_KEY, error),
        None => storage.delete_setting(WEB_UI_LAST_ERROR_KEY),
    });
    if let Err(error) = result {
        log::error!("Persist Web UI listener error: {error}");
    }
}

fn clear_last_error(storage: &Storage, state: &mut ControllerState) {
    state.last_error = None;
    persist_last_error(storage, None);
}

fn record_runtime_error(state: &mut ControllerState, error: WebUiError) -> WebUiError {
    state.last_error = Some(error.message.clone());
    error
}

fn concrete_listener_addresses(bound_ip: IpAddr) -> Vec<IpAddr> {
    if !bound_ip.is_unspecified() {
        return vec![bound_ip];
    }

    let mut addresses = vec![LOOPBACK_BIND_IP];
    let probe = match bound_ip {
        IpAddr::V4(_) => ("0.0.0.0:0", "192.0.2.1:9"),
        IpAddr::V6(_) => ("[::]:0", "[2001:db8::1]:9"),
    };
    if let Ok(socket) = UdpSocket::bind(probe.0)
        && socket.connect(probe.1).is_ok()
        && let Ok(address) = socket.local_addr()
        && !address.ip().is_unspecified()
    {
        addresses.push(address.ip());
    }
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::net::TcpStream;

    fn free_port() -> u16 {
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .expect("reserve free port")
            .local_addr()
            .expect("read free port")
            .port()
    }

    fn config(enabled: bool, port: u16, host_policy: WebUiHostPolicy) -> WebUiConfig {
        WebUiConfig {
            enabled,
            port,
            host_policy,
            allowlist: Vec::new(),
        }
    }

    async fn accepts_connection(address: SocketAddr) -> bool {
        tokio::time::timeout(Duration::from_millis(250), TcpStream::connect(address))
            .await
            .is_ok_and(|result| result.is_ok())
    }

    fn storage(temp: &TempDir) -> Storage {
        Storage::init_at(temp.path().join("usage.db"), false).expect("open storage")
    }

    // @lat: [[web-ui-server-tests#Disabled listener owns no socket]]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn disabled_listener_owns_no_socket() {
        let temp = TempDir::new().expect("temp data directory");
        let storage = storage(&temp);
        let port = free_port();
        save_web_ui_config(&storage, config(false, port, WebUiHostPolicy::Allowlist))
            .expect("save disabled config");
        let controller = WebListenerController::new(Arc::new(WebServerState::default()));

        controller.initialize(&storage).await;

        assert!(!controller.status(&storage).await.running);
        assert!(!accepts_connection(SocketAddr::new(LOOPBACK_BIND_IP, port)).await);
    }

    // @lat: [[web-ui-server-tests#Failed listener transitions preserve last known good#Different-port rollback]]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn different_port_bind_failure_keeps_old_listener_and_config() {
        let temp = TempDir::new().expect("temp data directory");
        let storage = storage(&temp);
        let old_port = free_port();
        let new_port = free_port();
        let controller = WebListenerController::new(Arc::new(WebServerState::default()));
        let old_config = controller
            .apply_config(&storage, config(true, old_port, WebUiHostPolicy::Allowlist))
            .await
            .expect("enable old listener");
        let _blocker = TcpListener::bind((Ipv4Addr::LOCALHOST, new_port))
            .await
            .expect("occupy replacement port");

        let error = controller
            .apply_config(&storage, config(true, new_port, WebUiHostPolicy::Allowlist))
            .await
            .expect_err("replacement bind must fail");

        assert_eq!(error.code, crate::web_server::WebUiErrorCode::BindFailed);
        assert_eq!(controller.config(&storage).await.unwrap(), old_config);
        assert_eq!(load_web_ui_config(&storage).unwrap(), old_config);
        assert!(accepts_connection(SocketAddr::new(LOOPBACK_BIND_IP, old_port)).await);

        controller
            .apply_config(
                &storage,
                config(false, old_port, WebUiHostPolicy::Allowlist),
            )
            .await
            .expect("disable listener");
    }

    // @lat: [[web-ui-server-tests#Failed listener transitions preserve last known good#Same-port address rollback]]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_port_address_failure_rebinds_old_listener_and_keeps_config() {
        let temp = TempDir::new().expect("temp data directory");
        let storage = storage(&temp);
        let port = free_port();
        let controller = WebListenerController::new(Arc::new(WebServerState::default()));
        let old_config = controller
            .apply_config(&storage, config(true, port, WebUiHostPolicy::Allowlist))
            .await
            .expect("enable loopback listener");
        controller.set_bind_failure_hook(Arc::new(|address| {
            address.ip().is_unspecified().then(|| {
                std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    "injected bind failure",
                )
            })
        }));

        let error = controller
            .apply_config(&storage, config(true, port, WebUiHostPolicy::All))
            .await
            .expect_err("wildcard bind must fail");

        assert_eq!(error.code, crate::web_server::WebUiErrorCode::BindFailed);
        assert_eq!(controller.config(&storage).await.unwrap(), old_config);
        assert_eq!(load_web_ui_config(&storage).unwrap(), old_config);
        let status = controller.status(&storage).await;
        assert_eq!(status.bound_addr, Some(format!("127.0.0.1:{port}")));
        assert!(accepts_connection(SocketAddr::new(LOOPBACK_BIND_IP, port)).await);

        controller
            .apply_config(&storage, config(false, port, WebUiHostPolicy::Allowlist))
            .await
            .expect("disable listener");
    }
}
