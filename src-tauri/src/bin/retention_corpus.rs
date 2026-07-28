//! Unsupported maintainer entry point for the private retention corpus study.
//!
//! It intentionally avoids a command-line dependency: this is a narrow
//! repository-local protocol surface, not a product CLI.

use quill_lib::retention_study::{
    ApprovalRecord, DbstatRequest, ProfileRequest, ReplayRequest, ReportRequest, StudyCancellation,
    SyntheticSmokeRequest, measure_dbstat, profile_source, render_scrubbed_report,
    run_replay_matrix, run_synthetic_smoke,
};
use std::{env, fs::File, path::PathBuf, process};

fn required_path(args: &mut impl Iterator<Item = String>, name: &str) -> Result<PathBuf, String> {
    match args.next().as_deref() {
        Some(flag) if flag == name => args
            .next()
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} needs a path")),
        _ => Err(format!("expected required {name} path")),
    }
}

fn read_approval(path: PathBuf) -> Result<ApprovalRecord, String> {
    serde_json::from_reader(
        File::open(&path).map_err(|error| format!("open approval record: {error}"))?,
    )
    .map_err(|error| format!("parse approval record: {error}"))
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let command = args.next().ok_or_else(|| {
        "missing subcommand: profile | replay | render-report | synthetic-smoke | dbstat"
            .to_string()
    })?;
    match command.as_str() {
        "profile" => {
            let approval = read_approval(required_path(&mut args, "--approval")?)?;
            let source = required_path(&mut args, "--source")?;
            let workspace = required_path(&mut args, "--workspace")?;
            let marker = required_path(&mut args, "--cancel-marker")?;
            if args.next().is_some() {
                return Err("unexpected profile argument".into());
            }
            let manifest = profile_source(ProfileRequest {
                approval,
                source: &source,
                workspace: &workspace,
                cancellation: StudyCancellation::from_marker(&marker),
            })
            .map_err(|error| error.to_string())?;
            println!("profiled private manifest: {}", manifest.display());
        }
        "replay" => {
            let manifest = required_path(&mut args, "--manifest")?;
            let workspace = required_path(&mut args, "--workspace")?;
            let marker = required_path(&mut args, "--cancel-marker")?;
            if args.next().is_some() {
                return Err("unexpected replay argument".into());
            }
            let result = run_replay_matrix(ReplayRequest {
                manifest: &manifest,
                workspace: &workspace,
                cancellation: StudyCancellation::from_marker(&marker),
            })
            .map_err(|error| error.to_string())?;
            println!("replay lifecycle: {}", result.lifecycle);
        }
        "render-report" => {
            let manifest = required_path(&mut args, "--manifest")?;
            let output = required_path(&mut args, "--output")?;
            let signoff = match args.next().as_deref() {
                Some("--privacy-signoff") => true,
                _ => return Err("render-report requires --privacy-signoff".into()),
            };
            if args.next().is_some() {
                return Err("unexpected render-report argument".into());
            }
            render_scrubbed_report(ReportRequest {
                manifest: &manifest,
                output: &output,
                privacy_signoff: signoff,
            })
            .map_err(|error| error.to_string())?;
            println!("wrote scrubbed report: {}", output.display());
        }
        "synthetic-smoke" => {
            let workspace = required_path(&mut args, "--workspace")?;
            if args.next().is_some() {
                return Err("unexpected synthetic-smoke argument".into());
            }
            for check in run_synthetic_smoke(SyntheticSmokeRequest {
                workspace: &workspace,
            })
            .map_err(|error| error.to_string())?
            {
                println!("PASS {check}");
            }
        }
        "dbstat" => {
            let manifest = required_path(&mut args, "--manifest")?;
            let scratch = required_path(&mut args, "--scratch")?;
            let marker = required_path(&mut args, "--cancel-marker")?;
            if args.next().is_some() {
                return Err("unexpected dbstat argument".into());
            }
            for (name, bytes) in measure_dbstat(DbstatRequest {
                manifest: &manifest,
                scratch: &scratch,
                cancellation: StudyCancellation::from_marker(&marker),
            })
            .map_err(|error| error.to_string())?
            {
                println!("{name}: {bytes}");
            }
        }
        _ => {
            return Err(
                "unknown subcommand: profile | replay | render-report | synthetic-smoke | dbstat"
                    .into(),
            );
        }
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("retention_corpus: {error}");
        process::exit(2);
    }
}
