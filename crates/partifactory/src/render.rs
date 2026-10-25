//! Browser checks and their isolated TLS content listener share one service.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rcgen::PublicKeyData;
use sha2::{Digest, Sha256};
use std::{path::Path, sync::Arc};

pub struct Renderer {
    pub service: Arc<pf_render::RenderService>,
    pub tls: Option<pf_serve::tls::Config>,
}
impl Renderer {
    pub fn new(
        store: &crate::store::Store,
        loaded: &pf_config::Loaded,
        env: &pf_config::Env,
    ) -> Result<Self, String> {
        let enabled = loaded.site.render.enabled;
        let tls = if enabled {
            let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
            let names = vec![format!("*.{}", loaded.site.domain.apex)];
            let params = rcgen::CertificateParams::new(names).map_err(|e| e.to_string())?;
            let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
            let pin = STANDARD.encode(Sha256::digest(key.subject_public_key_info()));
            let dir = env
                .get("PF_RENDER_PUBLIC_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| store.data_dir.join("render-public"));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            // Only public material leaves this process. The private key lives in memory.
            publish(&dir.join("spki"), pin.as_bytes())?;
            publish(&dir.join("certificate.der"), cert.der().as_ref())?;
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
                )
                .map_err(|e| e.to_string())?;
            let address = env
                .get("PF_LISTEN_RENDER")
                .map(String::as_str)
                .unwrap_or("0.0.0.0:443")
                .parse()
                .map_err(|_| "PF_LISTEN_RENDER must be an IP:port".to_string())?;
            Some(pf_serve::tls::Config {
                address,
                certificate: Arc::new(config),
            })
        } else {
            None
        };
        let mut cfg = pf_render::Config {
            enabled,
            ..Default::default()
        };
        if let Some(endpoint) = env.get("PF_RENDER_CDP") {
            cfg.endpoint = endpoint.clone();
        }
        if let Some(endpoint) = env.get("PF_RENDER_CONTROL") {
            cfg.control_endpoint = Some(endpoint.clone());
        }
        Ok(Self {
            service: Arc::new(pf_render::RenderService::new(
                store.parts.clone(),
                store.host.clone(),
                cfg,
            )),
            tls,
        })
    }
}
fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let pending = path.with_extension("pending");
    std::fs::write(&pending, bytes)
        .and_then(|_| std::fs::rename(pending, path))
        .map_err(|e| e.to_string())
}
pub struct Screenshots(pub Arc<pf_render::RenderService>);
impl pf_api::ScreenshotSource for Screenshots {
    fn get<'a>(
        &'a self,
        principal: &'a pf_model::Principal,
        id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<u8>, pf_model::PfError>> + Send + 'a>,
    > {
        Box::pin(self.0.screenshot(principal, id))
    }
}
