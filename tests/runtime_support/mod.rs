/// ONNX's static initializer can emit this exact nonfatal diagnostic in ARM64
/// Linux VMs, before open-why starts. Preserve all application diagnostics and
/// unknown vendor output; permit only one audited prefix on that configuration.
pub fn application_diagnostics(stderr: &str) -> &str {
    if cfg!(all(
        feature = "local-embeddings",
        target_os = "linux",
        target_arch = "aarch64"
    )) {
        stderr
            .strip_prefix(
                "onnxruntime cpuid_info warning: Unknown CPU vendor. cpuinfo_vendor value: 0\n",
            )
            .unwrap_or(stderr)
    } else {
        stderr
    }
}

#[test]
fn runtime_exception_is_exact_once_and_does_not_hide_application_output() {
    let warning = "onnxruntime cpuid_info warning: Unknown CPU vendor. cpuinfo_vendor value: 0\n";
    let input = format!("{warning}application failure");
    let expected = if cfg!(all(
        feature = "local-embeddings",
        target_os = "linux",
        target_arch = "aarch64"
    )) {
        "application failure"
    } else {
        &input
    };
    assert_eq!(application_diagnostics(&input), expected);
    let repeated = format!("{warning}{warning}");
    assert!(!application_diagnostics(&repeated).is_empty());
    let unknown = warning.replace("value: 0", "value: 1");
    assert_eq!(application_diagnostics(&unknown), unknown);
}
