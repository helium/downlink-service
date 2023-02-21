use config::{Config, Environment, File};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Settings {
    /// RUST_LOG compatible settings string. Default to INFO
    #[serde(default = "default_log")]
    pub log: String,
    /// Listen address for http requests. Default "0.0.0.0:80"
    #[serde(default = "default_http_listen_addr")]
    pub http_listen: SocketAddr,
    /// Listen address for grpc requests. Default "0.0.0.0:50051"
    #[serde(default = "default_grpc_listen_addr")]
    pub grpc_listen: SocketAddr,
    /// Listen address for metrics requests. Default "0.0.0.0:9000"
    #[serde(default = "default_metrics_listen_addr")]
    pub metrics_listen: SocketAddr,
    /// B58 Public key list (key1,key2) If absent a default is calculated
    /// by application code
    pub authorized_keys: Option<String>,
}

pub fn default_log() -> String {
    "INFO".to_string()
}

pub fn default_http_listen_addr() -> SocketAddr {
    "0.0.0.0:80".parse().expect("invalid default socket addr")
}

pub fn default_grpc_listen_addr() -> SocketAddr {
    "0.0.0.0:50051"
        .parse()
        .expect("invalid default socket addr")
}

pub fn default_metrics_listen_addr() -> SocketAddr {
    "0.0.0.0:9000".parse().expect("invalid default socket addr")
}

impl Settings {
    /// Load Settings from a given path. Settings are loaded from a given
    /// optional path and can be overriden with environment variables.
    ///
    /// Environemnt overrides have the same name as the entries in the settings
    /// file in uppercase and prefixed with "HDS_". For example
    /// "HDS_LOG" will override the log setting.
    pub fn new<P: AsRef<Path>>(path: Option<P>) -> Result<Self, config::ConfigError> {
        let mut builder = Config::builder();

        if let Some(file) = path {
            // Add optional settings file
            let filename = file.as_ref().to_str().expect("file name");
            builder = builder.add_source(File::with_name(filename).required(false));
        }
        // Add in settings from the environment (with a prefix of APP)
        // Eg.. `MI_DEBUG=1 ./target/app` would set the `debug` key
        builder
            .add_source(Environment::with_prefix("hds").prefix_separator("_"))
            .build()
            .and_then(|config| config.try_deserialize())
    }
}
