pub mod commands;

/// Returns true when the process should run in CLI mode rather than opening
/// the Tauri GUI. Any recognised subcommand or help/version flag triggers CLI
/// mode; no arguments at all means GUI mode.
pub fn is_cli_invocation() -> bool {
    let first = std::env::args().nth(1);
    match first.as_deref() {
        None => false,
        // Clap's built-in flags
        Some("--help") | Some("-h") | Some("--version") | Some("-V") => true,
        // Known subcommands
        Some(
            "setup" | "unsetup" | "status" | "apply" | "unapply" | "mv" | "push" | "pr"
            | "stack" | "unstack" | "sync" | "syncs" | "recover"
            | "suggest-work-branch" | "help"
            // Internal hook callbacks
            | "mc-post-commit" | "work-post-commit",
        ) => true,
        // Unknown argument → let Clap handle it (prints an error)
        _ => true,
    }
}

pub fn run() {
    if let Err(code) = commands::execute() {
        std::process::exit(code);
    }
}
