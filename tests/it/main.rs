//! All integration tests, in one test binary.
//!
//! Each file here is a module, not a test target of its own: one binary is
//! linked (and checked by the OS on its first run) instead of one per file.
//! Run them with `cargo nextest run`, which starts every test in a process
//! of its own, so a test's environment (`common::isolate_env`,
//! `common::run::env`) is its own, as it was when each file was a binary.
//!
//! A new test file goes here, with its `mod` line below; [`layout`] fails
//! when one is missing.

mod common;

mod api_provider;
mod approve_pr;
mod ask;
mod behavior_probe;
mod commit_context;
mod commit_flow;
mod live_behavior;
mod live_claude;
mod live_providers;
mod live_untrusted;
mod m0_m1;
mod m2;
mod no_key_literals;
mod notes;
mod pr;
mod pr_list;
mod safe_read;
mod second_opinion;
mod test_search;
mod tui;
mod tui_all_done;
mod tui_code_folds;
mod tui_list_blocks;
mod tui_list_edge;
mod tui_list_files;
mod tui_list_layout;
mod tui_list_story;
mod tui_list_triage;
mod tui_mouse;
mod tui_scroll;
mod tui_setup;
mod v2_acceptance;
mod v2_finish;
mod v2_finish_sheet;
mod v2_keymap;
mod v2_loop;
mod v2_loop_unit;
mod v2_migrate;
mod v2_migrate_unit;
mod v2_overlays;
mod v2_prepare;
mod v2_prepare_unit;
mod v2_queue;
mod v2_queue_act;
mod v2_queue_render;
mod v2_start;
mod v2_start_unit;
mod v2_ui;
mod v2_unit;
mod v2_unit_answers;
mod v2_unit_tools;
mod v2_unit_view;
mod whole_files;

/// Every file in `tests/it` is a module of this binary, and no test file
/// sits directly in `tests/`, where cargo would make it a binary of its own.
#[test]
fn layout() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let main = std::fs::read_to_string(tests.join("it/main.rs")).unwrap();
    let rs = |dir: &std::path::Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".rs").map(str::to_string))
            .collect();
        names.sort();
        names
    };
    let missing: Vec<String> = rs(&tests.join("it"))
        .into_iter()
        .filter(|n| n != "main" && !main.contains(&format!("\nmod {n};\n")))
        .collect();
    assert!(
        missing.is_empty(),
        "add `mod <name>;` to tests/it/main.rs: {missing:?}"
    );
    let stray = rs(&tests);
    assert!(stray.is_empty(), "move these into tests/it/: {stray:?}");
}
