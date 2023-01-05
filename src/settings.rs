use config::{Config, Environment, File};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Settings {
    /// RUST_LOG compatible settings string. Default to INFO
    #[serde(default = "default_log")]
    pub log: String,
    /// Listen address for http requests. Default "0.0.0.0:80"
    #[serde(default = "default_http_listen_addr")]
    pub http_listen: String,
    /// Listen address for grpc requests. Default "0.0.0.0:50051"
    #[serde(default = "default_grpc_listen_addr")]
    pub grpc_listen: String,
    /// Listen address for metrics requests. Default "0.0.0.0:9000"
    #[serde(default = "default_metrics_listen_addr")]
    pub metrics_listen: String,
    /// B58 Public key list (key1,key2), Default ""
    #[serde(default = "default_authorized_keys")]
    pub authorized_keys: String,
}

pub fn default_log() -> String {
    "INFO".to_string()
}

pub fn default_http_listen_addr() -> String {
    "0.0.0.0:80".to_string()
}

pub fn default_grpc_listen_addr() -> String {
    "0.0.0.0:50051".to_string()
}

pub fn default_metrics_listen_addr() -> String {
    "0.0.0.0:9000".to_string()
}

pub fn default_authorized_keys() -> String {
    "".to_string()
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
            builder = builder
                .add_source(File::with_name(&file.as_ref().to_string_lossy()).required(false));
        }
        // Add in settings from the environment (with a prefix of APP)
        // Eg.. `MI_DEBUG=1 ./target/app` would set the `debug` key
        builder
            .add_source(Environment::with_prefix("HDS").separator("_"))
            .build()
            .and_then(|config| config.try_deserialize())
    }
}
