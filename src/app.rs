use axum::serve;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::web::server::{create_router, AppState};
use zju_connect_gui::backend::paths::resolve_app_dir;
use zju_connect_gui::backend::proxy::ProxyManager;
use zju_connect_gui::backend::relaunch_args::ElevatedRelaunchArgs;
use zju_connect_gui::backend::settings_store::UserSettingsStore;

pub struct App {
    manager: ProxyManager,
    settings: Arc<Mutex<UserSettingsStore>>,
    #[allow(dead_code)]
    runtime: tokio::runtime::Runtime,
    #[allow(dead_code)]
    relaunch: ElevatedRelaunchArgs,
    broadcaster: async_broadcast::Sender<zju_connect_gui::backend::proxy::ProxyEvent>,
    receiver: async_broadcast::Receiver<zju_connect_gui::backend::proxy::ProxyEvent>,
}

impl App {
    pub async fn new(relaunch: ElevatedRelaunchArgs) -> Result<Self, Box<dyn std::error::Error>> {
        let app_dir = resolve_app_dir()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        let settings = Arc::new(Mutex::new(UserSettingsStore::new(&app_dir)));

        let (mut sender, receiver) = async_broadcast::broadcast(100);
        sender.set_overflow(true);

        let bridge = Arc::new(WebBridge {
            sender: sender.clone(),
        });
        let manager = ProxyManager::new(app_dir.clone(), runtime.handle().clone());
        manager.set_ui(bridge);

        Ok(Self {
            manager,
            settings,
            runtime,
            relaunch,
            broadcaster: sender,
            receiver,
        })
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let state = AppState {
            manager: self.manager.clone(),
            settings_store: self.settings.clone(),
            broadcaster: self.broadcaster.clone(),
            receiver: self.receiver.clone(),
        };

        let app = create_router(state);

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let url = format!("http://127.0.0.1:{}", port);

        log::info!("Server listening on {}", url);
        open::that(&url)?;

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let _tray = crate::tray::TrayController::new(url.clone(), tx.clone()).ok();

        tokio::spawn(async move {
            serve(listener, app).await.unwrap();
            tx.send(()).await.unwrap();
        });

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = rx.recv() => {}
        }

        Ok(())
    }
}

pub struct WebBridge {
    sender: async_broadcast::Sender<zju_connect_gui::backend::proxy::ProxyEvent>,
}

impl zju_connect_gui::backend::proxy::UiBridge for WebBridge {
    fn emit_event(&self, event: zju_connect_gui::backend::proxy::ProxyEvent) {
        let _ = self.sender.try_broadcast(event);
    }

    fn show_window(&self) {
        // Open browser again if possible
    }
}
