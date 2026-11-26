use folia_core::lifecycle_diagnostics::LifecycleDump;

fn main() {
    println!("{}", LifecycleDump::build().to_json_string());
}
