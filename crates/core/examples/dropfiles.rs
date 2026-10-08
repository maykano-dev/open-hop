//! Test helper: drop files onto whatever is under the pointer.
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trace")).init();
    let files: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    println!("dropped: {}", openhop_core::platform::dnd::drop_files(&files));
}
