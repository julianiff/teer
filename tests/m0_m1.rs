//! M0: diff extraction and `teer --json`. M1: semantic unit classification.

mod common;

use std::collections::BTreeMap;
use std::process::Command;

use common::*;
use teer::review::model::{UnitKind, UnitStatus};
use teer::review::{BuildOptions, build_review};

#[test]
fn json_lists_changed_files_with_contents() {
    let env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("a.py", "def a():\n    return 1\n")
        .write("gone.txt", "bye\n")
        .commit("init");
    repo.write("a.py", "def a():\n    return 2\n")
        .write("new.rs", "fn n() {}\n")
        .remove("gone.txt");

    let out = Command::new(teer_bin())
        .args(["--json", "--no-agent", "-C"])
        .arg(repo.path())
        .env("TEER_DATA_DIR", env.path().join("data"))
        .env("TEER_CONFIG", env.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let files: BTreeMap<String, (serde_json::Value, serde_json::Value)> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_string(),
                (f["old_content"].clone(), f["new_content"].clone()),
            )
        })
        .collect();
    assert_eq!(files.len(), 3);
    assert_eq!(files["a.py"].0, "def a():\n    return 1\n");
    assert_eq!(files["a.py"].1, "def a():\n    return 2\n");
    assert!(files["new.rs"].0.is_null());
    assert_eq!(files["new.rs"].1, "fn n() {}\n");
    assert_eq!(files["gone.txt"].0, "bye\n");
    assert!(files["gone.txt"].1.is_null());
    assert_eq!(v["head"], "WORKTREE");
}

#[test]
fn base_mode_compares_branch_to_merge_base() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("a.py", "def a():\n    return 1\n")
        .commit("init");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("a.py", "def a():\n    return 2\n")
        .commit("feature work");
    // Uncommitted changes are not part of a --base review.
    repo.write("b.py", "def b():\n    pass\n");
    let review = build_review(
        &repo.path(),
        &BuildOptions {
            base: Some("main".into()),
            languages: None,
        },
    )
    .unwrap();
    assert_eq!(review.files.len(), 1);
    assert_eq!(
        review.files[0].new_content.as_deref(),
        Some("def a():\n    return 2\n")
    );
    assert_ne!(review.head, "WORKTREE");
}

#[test]
fn version_uses_full_name() {
    let out = Command::new(teer_bin()).arg("--version").output().unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).starts_with("teer: the essence of every revision.")
    );
}

fn classify(
    file: &str,
    old: &str,
    new: &str,
) -> BTreeMap<String, (UnitStatus, UnitKind, Option<String>)> {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write(file, old).commit("init");
    repo.write(file, new);
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    review
        .units
        .into_iter()
        .map(|u| (u.symbol, (u.status, u.kind, u.renamed_from)))
        .collect()
}

fn assert_fixture(
    got: BTreeMap<String, (UnitStatus, UnitKind, Option<String>)>,
    names: [&str; 8],
    add_method: &str,
) {
    let [
        change,
        remove,
        local_rename,
        reformat,
        renamed,
        renamed_from,
        added,
        unchanged,
    ] = names;
    let status = |s: &str| got.get(s).map(|g| g.0);
    assert_eq!(status(change), Some(UnitStatus::Changed), "{got:#?}");
    assert_eq!(status(remove), Some(UnitStatus::Removed), "{got:#?}");
    assert_eq!(
        status(local_rename),
        Some(UnitStatus::NoBehaviorChange),
        "{got:#?}"
    );
    assert_eq!(
        status(reformat),
        Some(UnitStatus::NoBehaviorChange),
        "{got:#?}"
    );
    assert_eq!(
        status(renamed),
        Some(UnitStatus::NoBehaviorChange),
        "{got:#?}"
    );
    assert_eq!(got[renamed].2.as_deref(), Some(renamed_from));
    assert_eq!(status(added), Some(UnitStatus::Added), "{got:#?}");
    assert_eq!(status(add_method), Some(UnitStatus::Changed), "{got:#?}");
    assert!(!got.contains_key(unchanged), "{got:#?}");
    assert!(
        !got.contains_key("Cart"),
        "class with only method changes: {got:#?}"
    );
    assert_eq!(got.len(), 7, "{got:#?}");
}

#[test]
fn python_fixture() {
    let got = classify("checkout.py", PY_OLD, PY_NEW);
    assert_eq!(got["Cart::add"].1, UnitKind::Method);
    assert_fixture(
        got,
        [
            "charge_order",
            "old_helper",
            "compute_total",
            "format_me",
            "vat",
            "tax",
            "new_helper",
            "unchanged",
        ],
        "Cart::add",
    );
}

#[test]
fn rust_fixture() {
    let got = classify("src/checkout.rs", RS_OLD, RS_NEW);
    assert_fixture(
        got,
        [
            "charge_order",
            "old_helper",
            "compute_total",
            "format_me",
            "vat",
            "tax",
            "new_helper",
            "unchanged",
        ],
        "Cart::add",
    );
}

#[test]
fn typescript_fixture() {
    let got = classify("src/checkout.ts", TS_OLD, TS_NEW);
    assert_fixture(
        got,
        [
            "chargeOrder",
            "oldHelper",
            "computeTotal",
            "formatMe",
            "vat",
            "tax",
            "newHelper",
            "unchanged",
        ],
        "Cart::add",
    );
}

#[test]
fn javascript_fixture() {
    let got = classify("src/checkout.js", &js_old(), &js_new());
    assert_fixture(
        got,
        [
            "chargeOrder",
            "oldHelper",
            "computeTotal",
            "formatMe",
            "vat",
            "tax",
            "newHelper",
            "unchanged",
        ],
        "Cart::add",
    );
}

#[test]
fn php_fixture() {
    let got = classify("src/Checkout.php", PHP_OLD, PHP_NEW);
    assert_eq!(got["Cart::add"].1, UnitKind::Method);
    assert_fixture(
        got,
        [
            "charge_order",
            "old_helper",
            "compute_total",
            "format_me",
            "vat",
            "tax",
            "new_helper",
            "unchanged",
        ],
        "Cart::add",
    );
}

#[test]
fn php_callers_and_flow() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("src/Checkout.php", PHP_OLD)
        .write(
            "src/Controller.php",
            "<?php\nclass CheckoutController {\n    public function store($request) {\n        return \\App\\Checkout\\charge_order($request->order);\n    }\n}\n",
        )
        .commit("init");
    repo.write("src/Checkout.php", PHP_NEW);
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let charge = review
        .units
        .iter()
        .find(|u| u.symbol == "charge_order")
        .unwrap();
    let mut callers = charge.callers.clone();
    callers.sort();
    assert_eq!(
        callers,
        vec![
            "src/Checkout.php::new_helper".to_string(),
            "src/Controller.php::CheckoutController::store".to_string()
        ]
    );
}

#[test]
fn call_graph_finds_callers_in_other_files() {
    let _env = isolate_env();
    let repo = FixtureRepo::new();
    repo.write("checkout.py", PY_OLD)
        .write("api.py", "from checkout import charge_order\n\ndef handle(req):\n    return charge_order(req.order)\n\ndef main():\n    handle(parse())\n")
        .commit("init");
    repo.write("checkout.py", PY_NEW);
    let review = build_review(&repo.path(), &BuildOptions::default()).unwrap();
    let charge = review
        .units
        .iter()
        .find(|u| u.symbol == "charge_order")
        .unwrap();
    let mut callers = charge.callers.clone();
    callers.sort();
    assert_eq!(
        callers,
        vec![
            "api.py::handle".to_string(),
            "checkout.py::new_helper".to_string()
        ]
    );
    // The graph reaches two hops up for the impact view.
    assert!(review.graph.nodes.iter().any(|n| n.id == "api.py::main"));
}
