#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(std::ffi::OsStr::new("--init-database")) {
        let Some(path) = args.next().map(std::path::PathBuf::from) else {
            eprintln!("usage: quill --init-database PATH");
            std::process::exit(2);
        };
        if args.next().is_some() {
            eprintln!("usage: quill --init-database PATH");
            std::process::exit(2);
        }
        if let Err(error) = quill_lib::initialize_database(&path) {
            eprintln!("Failed to initialize database: {error}");
            std::process::exit(2);
        }
        return;
    }
    quill_lib::run()
}
