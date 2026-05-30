//! Render a violation into a shareable finding (markdown) and a repro file path.
use crate::runner::StepViolation;
use crate::scenario::Scenario;

pub fn render_markdown(v: &StepViolation, s: &Scenario) -> String {
    let json = serde_json::to_string_pretty(s).unwrap();
    format!(
        "# perp-adversary finding\n\n\
         **Broken at:** step {}\n\n\
         **Detail:** {}\n\n\
         ## Minimal scenario\n\n```json\n{}\n```\n\n\
         ## Repro\n\n```bash\ncargo run --bin replay -- scenarios/finding.json\n```\n",
        v.step, v.detail, json
    )
}
