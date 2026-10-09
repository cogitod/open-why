#[allow(dead_code)]
#[path = "../examples/embedded_adapter.rs"]
mod example;
#[allow(dead_code)]
mod onboarding_support;

#[test]
fn public_adapter_example_runs_without_models_or_private_services() {
    let s = onboarding_support::Sandbox::new();
    let directory = s.0.join("adapter");
    example::run(&directory).unwrap();
    let before = onboarding_support::snapshot(&directory);
    assert!(example::run(&directory).is_err());
    assert_eq!(onboarding_support::snapshot(&directory), before);
}
