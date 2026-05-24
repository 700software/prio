fn main() {
    if prio_lib::cli::is_cli_invocation() {
        prio_lib::cli::run();
    } else {
        #[cfg(all(windows, not(debug_assertions)))]
        prio_lib::hide_console_for_gui();
        prio_lib::run();
    }
}
