fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    if let Err(e) = slint_build::compile_with_config("ui/settings.slint", config) {
        panic!("compiling settings UI: {e}");
    }
}
