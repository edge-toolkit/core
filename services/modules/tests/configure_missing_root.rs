//! Covers `configure`'s fail-fast panic when `config.root` names a module that isn't among the
//! scanned `config.paths` -- a misconfiguration that must surface at startup, not as a silent 404.
#![cfg(test)]

use actix_web::{App, test};
use et_modules_service::{ModulesConfig, configure};

#[actix_rt::test]
#[should_panic(expected = "Root module 'nonexistent-root' not found")]
async fn configure_panics_when_root_module_is_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let config = ModulesConfig::new(vec![tmp.path().to_path_buf()], "nonexistent-root".to_string());

    let _app = test::init_service(App::new().configure(|cfg| configure(cfg, &config))).await;
}
