mod windows_tun;

fn main() -> std::process::ExitCode {
    match windows_tun::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            std::process::ExitCode::FAILURE
        }
    }
}
