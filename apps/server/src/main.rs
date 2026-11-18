use dagaz_server::{
    Config, create_s3, new_pairing_code, open_database, open_state, router, run_jobs,
};
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
    let state = open_state(config, db.clone(), s3).await?;
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!("Dagaz listening on {}", listener.local_addr()?);
    let (stop_send, mut stop) = tokio::sync::watch::channel(false);
    let mut worker = tokio::spawn(run_jobs(state, stop.clone()));
    let http_stop = stop_send.clone();
    let mut server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            tokio::select! {_=shutdown_signal()=>{let _=http_stop.send(true);},_=stop.changed()=>{}}
        })
        .await
    });
    tokio::select! {
        result=&mut server=>{let _=stop_send.send(true);result??;worker.await??;},
        result=&mut worker=>{let _=stop_send.send(true);server.await??;result??;},
    }
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
