//! kazoo-mcp on its own: the same instrument `kazoo mcp` runs.

fn main() -> std::process::ExitCode {
    let installed = kazoo_host::reports::install();
    let result = installed.and_then(|()| kazoo_mcp::cli(std::env::args().skip(1)));
    report(result)
}

fn report(result: color_eyre::Result<()>) -> std::process::ExitCode {
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(why) => {
            let invalid_input = why
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::InvalidInput);
            eprintln!("kazoo-mcp: {why}");
            if invalid_input {
                std::process::ExitCode::from(2)
            } else {
                std::process::ExitCode::FAILURE
            }
        }
    }
}
