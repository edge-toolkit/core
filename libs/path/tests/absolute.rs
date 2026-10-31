//! Covers `absolute_from`: a path already in normal form comes back exactly as the platform spells it.
//!
//! Compared as strings, since two `Path`s compare equal component by component, whichever separators they use.
#![cfg(test)]

use std::env;
use std::path::Path;

use et_path::absolute_from;

#[test]
fn a_relative_path_joins_its_base_as_the_platform_would() {
    let base = env::temp_dir().join("scenario");
    assert_eq!(
        absolute_from(&base, Path::new("modules")).as_os_str(),
        base.join("modules").as_os_str()
    );
}

#[test]
fn an_absolute_path_is_kept_as_the_platform_spells_it() {
    let path = env::temp_dir().join("scenario").join("modules");
    assert_eq!(
        absolute_from(Path::new("elsewhere"), &path).as_os_str(),
        path.as_os_str()
    );
}
