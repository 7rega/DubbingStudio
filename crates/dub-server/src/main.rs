//! Точка входа dub-server: поднимает axum на 127.0.0.1:8765 (порт как у backend/app.py).
//! Корень репо резолвится из env DUB_STUDIO_ROOT, иначе — рабочий каталог процесса.

use dub_server::{apply_proxy_env, augment_path_for_tools, build_router, AppState};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let repo_root = std::env::var("DUB_STUDIO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().expect("cwd"));

    let port: u16 = std::env::var("DUB_STUDIO_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8765);

    // Прописать в PATH каталоги скачанных бинарей (ffmpeg/llama/higgs-engine) до старта — чтобы после
    // автозакачки они находились без рестарта процесса.
    augment_path_for_tools(&repo_root);
    apply_proxy_env(&repo_root);

    let state = AppState::new(&repo_root);
    tracing::info!(
        "dub-server: repo_root={}, workspace={}, web_root={:?}",
        state.repo_root.display(),
        state.workspace.display(),
        state.web_root
    );

    let app = build_router(state);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("слушаю http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
