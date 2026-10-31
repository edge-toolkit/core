//! Covers the `modules.rs` source a firmware build writes: which files of which packages it builds in, how each is
//! served, and every way a module selection is refused.
#![cfg(test)]

use std::path::{Path, PathBuf};

use et_firmware_modules::{Generated, ModulesError, generate, include_path};
use fs_err as fs;
use tempfile::{TempDir, tempdir};

/// Write `contents` to `path` under `root`, making its directories.
fn write(root: &Path, path: &str, contents: &str) -> PathBuf {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

/// Make `link` a symbolic link to the file `target`.
fn link_to(target: &Path, link: &Path) -> PathBuf {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(target, link).unwrap();
    link.to_path_buf()
}

/// A built module in `pkg/`, listing its files, beside a source module that lists none.
fn two_modules() -> TempDir {
    let root = tempdir().unwrap();
    let listed = r#"{"name": "@scope/built", "files": ["app.js", "assets", "./index.html"]}"#;
    let _manifest = write(root.path(), "built/pkg/package.json", listed);
    let _app = write(root.path(), "built/pkg/app.js", "app");
    let _page = write(root.path(), "built/pkg/index.html", "<p>page</p>");
    let _model = write(root.path(), "built/pkg/assets/model.wasm", "\0asm");
    let _hidden = write(root.path(), "built/pkg/assets/.cache", "hidden");
    let _unlisted = write(root.path(), "built/pkg/notes.md", "unlisted");
    let _source = write(root.path(), "built/src/lib.rs", "outside pkg");
    let _manifest = write(root.path(), "plain/package.json", r#"{"name": "plain"}"#);
    let _style = write(root.path(), "plain/style.css", "p {}");
    let _dependency = write(root.path(), "plain/node_modules/dep/index.js", "dependency");
    root
}

/// The paths of the two modules, as `MODULES_PATHS` lists them.
fn paths(root: &TempDir) -> String {
    format!(
        "{}, {} ,",
        root.path().join("built").display(),
        root.path().join("plain").display()
    )
}

/// The source `generate` returns for `paths`, which has to succeed.
fn generated(paths: &str, root: &str, elsewhere: &str, url: &str) -> Generated {
    generate(paths, root, elsewhere, url).unwrap()
}

#[test]
fn it_builds_in_what_each_package_publishes() {
    let root = two_modules();
    let generated = generated(
        &paths(&root),
        "@scope/built",
        "big, cdn-only",
        "https://cdn.example/npm",
    );
    let names: Vec<String> = generated
        .files
        .iter()
        .map(|file| {
            file.strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(
        names,
        [
            "built/pkg/app.js",
            "built/pkg/assets/model.wasm",
            "built/pkg/index.html",
            "built/pkg/package.json",
            "plain/package.json",
            "plain/style.css",
        ]
    );

    let source = &generated.source;
    assert!(source.contains(r#"Module::new("@scope/built", &["#));
    assert!(
        source.contains(r#"("assets/model.wasm", et_ws_server::picoserve_modules::ModuleFile::new("application/wasm""#)
    );
    assert!(
        source
            .contains(r#"("index.html", et_ws_server::picoserve_modules::ModuleFile::new("text/html; charset=utf-8""#)
    );
    assert!(source.contains(r#"Module::new("plain", &["#));
    assert!(source.contains("    \"@scope/built\",\n"));
    assert!(source.contains(r#"&["big", "cdn-only"],"#));
    assert!(source.contains(r#""https://cdn.example/npm","#));
}

#[test]
fn a_file_is_tagged_by_its_contents() {
    let root = tempdir().unwrap();
    let _manifest = write(
        root.path(),
        "m/package.json",
        r#"{"name": "m", "files": ["a.txt", "b.txt", "c.txt"]}"#,
    );
    let _a = write(root.path(), "m/a.txt", "same");
    let _b = write(root.path(), "m/b.txt", "same");
    let _c = write(root.path(), "m/c.txt", "different");
    let source = generated(&root.path().join("m").display().to_string(), "", "", "").source;
    let tags: Vec<&str> = source
        .lines()
        .filter(|line| line.contains(".txt\""))
        .map(|line| line.rsplit_once(", \"").unwrap().1)
        .collect();
    assert_eq!(tags.len(), 3);
    assert_eq!(tags[0], tags[1]);
    assert_ne!(tags[0], tags[2]);
}

#[test]
fn each_file_is_served_as_its_extension_says() {
    let root = tempdir().unwrap();
    let extensions = [
        ("htm", "text/html; charset=utf-8"),
        ("mjs", "application/javascript; charset=utf-8"),
        ("map", "application/json"),
        ("png", "image/png"),
        ("svg", "image/svg+xml"),
        ("ico", "image/x-icon"),
        ("py", "text/plain; charset=utf-8"),
        ("bin", "application/octet-stream"),
    ];
    let _manifest = write(root.path(), "m/package.json", r#"{"name": "m"}"#);
    for (extension, _) in extensions {
        let _file = write(root.path(), &format!("m/file.{extension}"), extension);
    }
    let _bare = write(root.path(), "m/LICENSE", "no extension");
    let source = generated(&root.path().join("m").display().to_string(), "", "", "").source;
    for (extension, content_type) in extensions {
        assert!(source.contains(&format!(
            r#"("file.{extension}", et_ws_server::picoserve_modules::ModuleFile::new("{content_type}""#
        )));
    }
    assert!(
        source.contains(r#"("LICENSE", et_ws_server::picoserve_modules::ModuleFile::new("application/octet-stream""#)
    );
}

#[test]
fn nothing_named_builds_in_nothing() {
    let generated = generated(" , ", "", "", "");
    assert_eq!(generated.files, Vec::<PathBuf>::new());
    assert!(
        generated
            .source
            .contains("Modules::new(\n    &[\n    ],\n    \"\",\n    &[],\n    \"\",\n);")
    );
}

#[test]
fn a_selection_that_cannot_be_served_is_refused() {
    let root = two_modules();
    let unknown_root = generate(&paths(&root), "absent", "", "").unwrap_err();
    assert!(matches!(&unknown_root, ModulesError::UnknownRoot { root, names } if root == "absent" && names.len() == 2));
    let no_url = generate("", "", "big", "").unwrap_err();
    assert!(matches!(no_url, ModulesError::ElsewhereWithoutUrl));
    let relative = generate("relative/module", "", "", "").unwrap_err();
    assert!(matches!(relative, ModulesError::RelativePath { .. }));
}

#[test]
fn a_link_out_of_the_package_is_refused() {
    let root = tempdir().unwrap();
    let secret = write(root.path(), "secret.txt", "secret");
    let _manifest = write(root.path(), "m/package.json", r#"{"name": "m"}"#);
    let _inside = write(root.path(), "m/inside.txt", "inside");
    let link = link_to(&secret, &root.path().join("m/leak.txt"));
    let refused = generate(&root.path().join("m").display().to_string(), "", "", "").unwrap_err();
    assert!(matches!(&refused, ModulesError::LinksOutside(file) if *file == link));
}

#[test]
fn a_link_within_the_package_is_built_in() {
    let root = tempdir().unwrap();
    let _manifest = write(root.path(), "m/package.json", r#"{"name": "m"}"#);
    let inside = write(root.path(), "m/inside.txt", "inside");
    let link = link_to(&inside, &root.path().join("m/alias.txt"));
    let generated = generated(&root.path().join("m").display().to_string(), "", "", "");
    assert!(generated.files.contains(&link));
}

#[test]
fn a_package_that_cannot_be_read_is_refused() {
    let root = tempdir().unwrap();
    let cases = [
        ("missing", None),
        ("not-json", Some("{")),
        ("nameless", Some(r#"{"files": []}"#)),
        ("odd-file", Some(r#"{"name": "odd", "files": [7]}"#)),
        ("escapes", Some(r#"{"name": "escapes", "files": ["/secret"]}"#)),
    ];
    for (dir, manifest) in cases {
        fs::create_dir_all(root.path().join(dir)).unwrap();
        if let Some(manifest) = manifest {
            let _manifest = write(root.path(), &format!("{dir}/package.json"), manifest);
        }
        let refused = generate(&root.path().join(dir).display().to_string(), "", "", "").unwrap_err();
        let expected = match dir {
            "missing" => matches!(refused, ModulesError::Io(_)),
            "not-json" => matches!(refused, ModulesError::Json { .. }),
            "nameless" => matches!(refused, ModulesError::NoName(_)),
            "odd-file" => matches!(refused, ModulesError::FileNotString(_)),
            _ => matches!(&refused, ModulesError::OutsidePackage { entry, .. } if entry == "/secret"),
        };
        assert!(expected, "{dir}: {refused}");
    }
}

#[test]
fn an_include_path_is_absolute_and_utf8() {
    let root = tempdir().unwrap();
    let literal = include_path("VAR", root.path()).unwrap();
    assert_eq!(literal, format!("{:?}", root.path().to_str().unwrap()));

    let relative = include_path("VAR", Path::new("relative")).unwrap_err();
    assert_eq!(relative.to_string(), "VAR must hold absolute paths, not relative");

    // A lone byte that starts no UTF-8 sequence, or a lone surrogate, as each platform spells a name.
    #[cfg(unix)]
    let name: std::ffi::OsString = std::os::unix::ffi::OsStringExt::from_vec(vec![0xff]);
    #[cfg(windows)]
    let name: std::ffi::OsString = std::os::windows::ffi::OsStringExt::from_wide(&[0xD800]);
    let not_utf8 = include_path("VAR", &root.path().join(name)).unwrap_err();
    assert_eq!(not_utf8.to_string(), "VAR holds a path that is not valid UTF-8");
}
