use perp_adversary::driver::run;
use perp_adversary::oracles::check_observation;
use perp_adversary::scenario::Scenario;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: replay <scenario.json>");
    let s: Scenario = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let trace = run(&s);
    for obs in &trace.observations {
        match check_observation(obs) {
            Ok(()) => {}
            Err(v) => {
                eprintln!("VIOLATION at step {}: {}", obs.step, v.detail);
                std::process::exit(1);
            }
        }
    }
    println!("no violation across {} steps", trace.observations.len());
}
