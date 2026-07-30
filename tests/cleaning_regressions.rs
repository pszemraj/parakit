//! Public-facade smoke coverage for the shipped cleaning pipeline.

use parakit::rules::{build_cleaner, CleaningProfile};

#[test]
fn public_builder_runs_the_shipped_default_pipeline() {
    let cleaner = build_cleaner(false, CleaningProfile::Safe, true, None, &[], &[])
        .expect("cleaner should compile")
        .expect("cleaning should be enabled");

    assert_eq!(
        cleaner.clean_text("um, the the G P T model is gonna work."),
        "The GPT model is going to work"
    );
}
