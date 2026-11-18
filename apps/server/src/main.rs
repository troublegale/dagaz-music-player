use dagaz_server::{Config, build_app, create_s3, new_pairing_code, open_database};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !args.is_empty() && args != ["pair-code"] {
        return Err("usage: dagaz [pair-code]".into());
    }
    let config = Config::from_env()?;
    let db = open_database(&config).await?;
    if args == ["pair-code"] {
        println!("{}", new_pairing_code(&db).await?);
        db.close().await;
        return Ok(());
    }
    let s3 = create_s3(&config)?;
    let listen = config.listen;
    let app = build_app(config, db.clone(), s3).await?;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!("Dagaz listening on {}", listener.local_addr()?);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    db.close().await;
    Ok(())
}

async fn shutdown_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = interrupt => {}, _ = terminate => {} }
}
