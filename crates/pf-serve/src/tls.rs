//! Internal TLS uses the public content router verbatim, including its access/header layers.
use axum::{Extension, Router, extract::ConnectInfo};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::broadcast, task::JoinSet};

#[derive(Clone)]
pub struct Config {
    pub address: SocketAddr,
    pub certificate: Arc<rustls::ServerConfig>,
}

pub async fn serve(
    listener: TcpListener,
    config: Config,
    router: Router,
    mut shutdown: broadcast::Receiver<()>,
) -> std::io::Result<()> {
    let acceptor = tokio_rustls::TlsAcceptor::from(config.certificate);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            _ = connections.join_next(), if !connections.is_empty() => {},
            result = listener.accept() => {
                let (socket, peer) = result?;
                let acceptor = acceptor.clone();
                let router = router.clone().layer(Extension(ConnectInfo(peer)));
                connections.spawn(async move {
                    let Ok(Ok(stream)) = tokio::time::timeout(Duration::from_secs(5), acceptor.accept(socket)).await else { return; };
                    let _ = Builder::new(TokioExecutor::new()).serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(router)).await;
                });
            }
        }
    }
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    Ok(())
}
