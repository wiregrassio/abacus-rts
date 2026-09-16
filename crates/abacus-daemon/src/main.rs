use std::path::PathBuf;

fn main() {
    let socket_path = parse_args();

    if let Err(e) = abacus_daemon::daemon::daemon_run(&socket_path) {
        eprintln!("abacus: fatal: {e}");
        std::process::exit(1);
    }
}

fn parse_args() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();

    for arg in &args[1..] {
        if let Some(path) = arg.strip_prefix("--socket-path=") {
            return PathBuf::from(path);
        }
    }

    // Two-arg form: --socket-path /path
    let mut iter = args[1..].iter();
    while let Some(arg) = iter.next() {
        if arg == "--socket-path" {
            if let Some(path) = iter.next() {
                return PathBuf::from(path);
            }
        }
    }

    PathBuf::from("/run/abacus-rts/abacus.sock")
}
