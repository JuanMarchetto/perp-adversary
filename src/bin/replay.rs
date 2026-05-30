//! Replay a scenario JSON through the engine and check O1 at every step.
//!
//! Trust boundary: the scenario file is UNTRUSTED external input. Every failure
//! mode — missing path, unreadable file, malformed JSON, and out-of-range
//! account/asset indices — is handled as a recoverable error with a clear
//! message and a non-zero exit code, never a panic.
use perp_adversary::driver::run;
use perp_adversary::oracles::check_observation;
use perp_adversary::scenario::Scenario;
use std::process::ExitCode;

fn run_main() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| "usage: replay <scenario.json>".to_string())?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read scenario file {path:?}: {e}"))?;
    let s: Scenario = serde_json::from_str(&text)
        .map_err(|e| format!("malformed scenario JSON in {path:?}: {e}"))?;
    // Reject structurally-invalid scenarios (out-of-range indices, etc.) before
    // touching the engine, so untrusted input cannot panic the driver.
    s.validate()
        .map_err(|e| format!("invalid scenario in {path:?}: {e}"))?;

    let trace = run(&s);
    for obs in &trace.observations {
        if let Err(v) = check_observation(obs) {
            return Err(format!("VIOLATION at step {}: {}", obs.step, v.detail));
        }
    }
    println!("no violation across {} steps", trace.observations.len());
    Ok(())
}

fn main() -> ExitCode {
    match run_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}
