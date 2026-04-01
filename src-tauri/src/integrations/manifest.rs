use serde::{Deserialize, Serialize};

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OwnedAssetManifest {
    pub files: Vec<String>,
    pub directories: Vec<String>,
    pub config_keys: Vec<String>,
    pub markdown_blocks: Vec<String>,
}
