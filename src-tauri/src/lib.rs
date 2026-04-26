//! k8sdesk library entry — registers Tauri commands and shared state.

mod commands;
mod dashboard;
mod dsl;
mod error;
mod kube_client;
mod safety;
mod store;

use std::sync::Arc;

use tauri::Manager;
use tokio::sync::Mutex;

pub struct AppState {
    pub store: Mutex<store::EncryptedStore>,
    pub clients: Mutex<kube_client::ClientCache>,
    pub pending: Mutex<safety::PendingConfirmations>,
    pub dashboards: Mutex<dashboard::DashboardRegistry>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .setup(|app| {
            let app_dir = app
                .path()
                .app_data_dir()
                .expect("app data dir resolvable");
            std::fs::create_dir_all(&app_dir).ok();

            let store = store::EncryptedStore::open(&app_dir)
                .expect("failed to open encrypted store");

            let state = Arc::new(AppState {
                store: Mutex::new(store),
                clients: Mutex::new(kube_client::ClientCache::default()),
                pending: Mutex::new(safety::PendingConfirmations::default()),
                dashboards: Mutex::new(dashboard::DashboardRegistry::default()),
            });
            app.manage(state);
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            commands::cluster_list,
            commands::cluster_add,
            commands::cluster_update,
            commands::cluster_delete,
            commands::cluster_import_kubeconfig,
            commands::cluster_update_from_kubeconfig,
            commands::namespace_list,
            commands::dsl_execute,
            commands::dashboard_open,
            commands::dashboard_status,
            commands::dashboard_stop,
            commands::open_url,
            commands::cluster_folder_get,
            commands::cluster_folder_set,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
