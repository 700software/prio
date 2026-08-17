pub mod cli;
pub mod error;
pub mod git;
pub mod hooks;
pub mod logger;
pub mod result;
pub mod services;
pub mod storage;
pub mod util;

/// Hide the console window when launching the GUI on Windows release builds.
/// The binary uses the default console subsystem so CLI output works from a
/// terminal; without this, double-clicking the exe would flash a console.
pub fn hide_console_for_gui() {
    #[cfg(all(windows, not(debug_assertions)))]
    unsafe {
        extern "system" {
            fn GetConsoleWindow() -> isize;
            fn ShowWindow(hwnd: isize, n_cmd_show: i32) -> i32;
        }
        const SW_HIDE: i32 = 0;
        let hwnd = GetConsoleWindow();
        if hwnd != 0 {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

use std::path::PathBuf;

use logger::Logger;
use result::PrioResult;
use services::{abort, apply, cp, mv, pr, push, recover, setup, stack, status, sync, unsetup};
use storage::user_config;

fn run_service<F>(f: F) -> PrioResult
where
    F: FnOnce(&mut Logger) -> Result<PrioResult, error::PrioError>,
{
    let mut logger = Logger::ui();
    match f(&mut logger) {
        Ok(mut r) => {
            r.logs.extend(logger.drain());
            r
        }
        Err(e) => PrioResult::from_error(e, logger.drain()),
    }
}

#[tauri::command]
fn prio_setup(
    repo_path: String,
    mc_path: Option<String>,
    work_branch: Option<String>,
) -> PrioResult {
    run_service(|logger| {
        let interactive = work_branch.is_none();
        setup::run(
            Some(PathBuf::from(repo_path)),
            mc_path.map(PathBuf::from),
            work_branch,
            interactive,
            logger,
        )
    })
}

#[tauri::command]
fn prio_suggest_work_branch(repo_path: String) -> Result<setup::WorkBranchSuggestion, String> {
    setup::suggest_work_branch(&PathBuf::from(repo_path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn prio_status(repo_path: Option<String>) -> Result<status::StatusResult, String> {
    let mut logger = Logger::ui();
    status::run(repo_path.map(PathBuf::from), &mut logger).map_err(|e| e.to_string())
}

#[tauri::command]
fn prio_apply(repo_path: Option<String>, branches: Vec<String>) -> PrioResult {
    run_service(|logger| apply::run(repo_path.map(PathBuf::from), branches, false, logger))
}

#[tauri::command]
fn prio_unapply(repo_path: Option<String>, branches: Vec<String>) -> PrioResult {
    run_service(|logger| apply::run(repo_path.map(PathBuf::from), branches, true, logger))
}

#[tauri::command]
fn prio_reorder(repo_path: Option<String>, branches: Vec<String>) -> PrioResult {
    run_service(|logger| apply::reorder(repo_path.map(PathBuf::from), branches, logger))
}

#[tauri::command]
fn prio_mv(
    repo_path: Option<String>,
    commits: Vec<String>,
    destination: String,
    create: bool,
    apply: Option<bool>,
) -> PrioResult {
    run_service(|logger| {
        mv::run(
            repo_path.map(PathBuf::from),
            commits,
            destination,
            create,
            apply.unwrap_or(false),
            false, // force: UI never force-pushes; use CLI -f instead
            logger,
        )
    })
}

#[tauri::command]
fn prio_cp(
    repo_path: Option<String>,
    commits: Vec<String>,
    destination: String,
    create: bool,
    apply: Option<bool>,
) -> PrioResult {
    run_service(|logger| {
        cp::run(
            repo_path.map(PathBuf::from),
            commits,
            destination,
            create,
            apply.unwrap_or(false),
            logger,
        )
    })
}

#[tauri::command]
fn prio_push(repo_path: Option<String>, branch: String) -> PrioResult {
    run_service(|logger| push::run(repo_path.map(PathBuf::from), branch, false, logger))
}

#[tauri::command]
fn prio_pr(repo_path: Option<String>, branch: String) -> PrioResult {
    run_service(|logger| pr::run(repo_path.map(PathBuf::from), branch, logger))
}

#[tauri::command]
fn prio_stack(repo_path: Option<String>, branch: String, dependencies: Vec<String>) -> PrioResult {
    run_service(|logger| {
        stack::run_stack(repo_path.map(PathBuf::from), branch, dependencies, logger)
    })
}

#[tauri::command]
fn prio_unstack(repo_path: Option<String>, branch: String, keep: bool) -> PrioResult {
    run_service(|logger| {
        stack::run_unstack(repo_path.map(PathBuf::from), branch, keep, false, logger)
    })
}

#[tauri::command]
fn prio_sync(repo_path: Option<String>) -> PrioResult {
    run_service(|logger| sync::run(repo_path.map(PathBuf::from), logger))
}

#[tauri::command]
fn prio_syncs() -> PrioResult {
    run_service(|logger| sync::run_syncs(logger))
}

#[tauri::command]
fn prio_recover(repo_path: Option<String>) -> PrioResult {
    run_service(|logger| recover::run(repo_path.map(PathBuf::from), logger))
}

#[tauri::command]
fn prio_abort(repo_path: Option<String>) -> PrioResult {
    run_service(|logger| abort::run(repo_path.map(PathBuf::from), logger))
}

#[tauri::command]
fn prio_unsetup(repo_path: Option<String>) -> PrioResult {
    run_service(|logger| unsetup::run(repo_path.map(PathBuf::from), false, false, logger))
}

#[tauri::command]
fn prio_list_repos() -> Result<Vec<user_config::RepoRecord>, String> {
    user_config::load_repos().map_err(|e| e.to_string())
}

#[tauri::command]
fn prio_load_ui_state() -> Result<user_config::UiState, String> {
    user_config::load_ui_state().map_err(|e| e.to_string())
}

#[tauri::command]
fn prio_save_ui_state(tab_order: Vec<String>) -> Result<(), String> {
    user_config::save_ui_state(&user_config::UiState { tab_order }).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            prio_setup,
            prio_suggest_work_branch,
            prio_status,
            prio_apply,
            prio_unapply,
            prio_reorder,
            prio_mv,
            prio_cp,
            prio_push,
            prio_pr,
            prio_stack,
            prio_unstack,
            prio_sync,
            prio_syncs,
            prio_recover,
            prio_abort,
            prio_unsetup,
            prio_list_repos,
            prio_load_ui_state,
            prio_save_ui_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
